#![no_main]
#![no_std]

extern crate alloc;

use alloc::{vec, vec::Vec};
use core::mem::{forget, size_of};
use core::slice;

use eneros_boot_manager_uefi::{
    declared_manifest_len, manifest_digest, verify_component, verify_manifest, LoaderError,
    TrustPolicy, MANIFEST_MEDIA_OFFSET,
};
use eneros_boot_protocol::boot_info::{
    BOOT_FLAG_ENTROPY_VALID, BOOT_FLAG_MEASUREMENTS_VALID, BOOT_FLAG_RECOVERY,
    BOOT_FLAG_SECURE_BOOT, BOOT_FLAG_TRIAL,
};
use eneros_boot_protocol::{
    condition_boot_entropy, dtb_window_for, resolve_boot_control, BootControl, BootControlAnchor,
    BootControlTransaction, BootInfo, BootReason, BootSelection, BootSlot, Component,
    ComponentRole, IdentityCapsule, ManifestSlot, MeasurementKind, MeasurementLog, MemoryMapEntry,
    MemoryRegionKind, ReleaseManifest, ResolvedBootControl, BOOT_CONTROL_ANCHOR_SIZE,
    BOOT_CONTROL_COPY_OFFSETS, BOOT_CONTROL_FLAG_MANUAL_RECOVERY, BOOT_CONTROL_RECORD_SIZE,
    ENEROS_DTB_MAX_LEN, ENEROS_INIT_BUNDLE_PHYS, IDENTITY_CAPSULE_PHYS, MANIFEST_HEADER_LEN,
    MEASUREMENT_LOG_SIZE, RAW_BOOT_ENTROPY_SIZE,
};
use eneros_image_format::{
    read_gpt, BlockDevice, BlockReadError, Gpt, Guid, ParsedPartition, PartitionReader,
};
use sha2::{Digest, Sha256};
use uefi::boot::{self, AllocateType, MemoryType, ScopedProtocol, SearchType};
use uefi::mem::memory_map::MemoryMap;
use uefi::proto::media::block::BlockIO;
use uefi::proto::rng::Rng;
use uefi::runtime::{self, ResetType, VariableAttributes, VariableVendor};
use uefi::{entry, println, Status};

include!(concat!(env!("OUT_DIR"), "/trust_anchor.rs"));
// BOOT-001：磁盘 GUID 由构建期注入（build.rs 从 ENEROS_DISK_GUID_HEX 生成；
// 旧硬编码常量删除，避免与镜像工厂 GUID 漂移）。
include!(concat!(env!("OUT_DIR"), "/expected_disk_guid.rs"));

const EXPECTED_DISK_GUID: Guid = Guid(EXPECTED_DISK_GUID_BYTES);
const MAX_MEMORY_MAP_ENTRIES: usize = 128;
const MINIMUM_COMPILED_SECURITY_VERSION: u64 = 1;
const BOOT_CONTROL_VENDOR: VariableVendor =
    VariableVendor(uefi::guid!("454e4552-4f53-4001-8000-0000000000bc"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BootFailure {
    NoBlockDevice,
    Media,
    DiskIdentity,
    PartitionMissing,
    PartitionRead,
    Manifest(LoaderError),
    ComponentMissing,
    Allocation,
    NoDeviceTree,
    EntropyUnavailable,
    EntropyFailure,
    BootControl,
    ProtectedState,
    BootControlWrite,
    Measurement,
    /// WP-27.2：DTB 装配布局不一致（bundle header 与 manifest 长度不符 /
    /// DTB 超窗口）——fail-closed，不静默截断。
    DtLayout,
}

impl BootFailure {
    const fn code(self) -> &'static str {
        match self {
            Self::NoBlockDevice => "NO_BLOCK_DEVICE",
            Self::Media => "MEDIA",
            Self::DiskIdentity => "GPT_OR_DISK_IDENTITY",
            Self::PartitionMissing => "PARTITION_MISSING",
            Self::PartitionRead => "PARTITION_READ",
            Self::Manifest(error) => error.code(),
            Self::ComponentMissing => "COMPONENT_MISSING",
            Self::Allocation => "ALLOCATION",
            Self::NoDeviceTree => "NO_DEVICE_TREE",
            Self::EntropyUnavailable => "ENTROPY_UNAVAILABLE",
            Self::EntropyFailure => "ENTROPY_FAILURE",
            Self::BootControl => "BOOTCTRL",
            Self::ProtectedState => "PROTECTED_STATE",
            Self::BootControlWrite => "BOOTCTRL_WRITE",
            Self::Measurement => "MEASUREMENT",
            Self::DtLayout => "DT_LAYOUT",
        }
    }
}

struct UefiDisk {
    protocol: ScopedProtocol<BlockIO>,
    sector_size: u32,
    total_lbas: u64,
    media_id: u32,
    reads: u64,
    writes: u64,
}

impl BlockDevice for UefiDisk {
    fn sector_size(&self) -> u32 {
        self.sector_size
    }

    fn total_lbas(&self) -> u64 {
        self.total_lbas
    }

    fn read_blocks(&mut self, first_lba: u64, buffer: &mut [u8]) -> Result<(), BlockReadError> {
        let sector = usize::try_from(self.sector_size).map_err(|_| BlockReadError::SectorSize)?;
        if buffer.is_empty() || buffer.len() % sector != 0 {
            return Err(BlockReadError::BufferSize);
        }
        let blocks = u64::try_from(buffer.len() / sector).map_err(|_| BlockReadError::Overflow)?;
        if first_lba
            .checked_add(blocks)
            .ok_or(BlockReadError::Overflow)?
            > self.total_lbas
        {
            return Err(BlockReadError::OutOfBounds);
        }
        self.protocol
            .read_blocks(self.media_id, first_lba, buffer)
            .map_err(|_| BlockReadError::Media)?;
        self.reads = self
            .reads
            .checked_add(blocks)
            .ok_or(BlockReadError::Overflow)?;
        Ok(())
    }
}

impl UefiDisk {
    fn write_control_copy(
        &mut self,
        partition: &ParsedPartition,
        offset: u64,
        record: &[u8; BOOT_CONTROL_RECORD_SIZE],
    ) -> Result<(), BootFailure> {
        let sector = u64::from(self.sector_size);
        if offset % sector != 0
            || u64::try_from(record.len()).map_err(|_| BootFailure::Media)? % sector != 0
        {
            return Err(BootFailure::BootControlWrite);
        }
        let partition_bytes = partition
            .last_lba
            .checked_sub(partition.first_lba)
            .and_then(|value| value.checked_add(1))
            .and_then(|value| value.checked_mul(sector))
            .ok_or(BootFailure::Media)?;
        if offset
            .checked_add(u64::try_from(record.len()).map_err(|_| BootFailure::Media)?)
            .filter(|end| *end <= partition_bytes)
            .is_none()
        {
            return Err(BootFailure::BootControlWrite);
        }
        let lba = partition
            .first_lba
            .checked_add(offset / sector)
            .ok_or(BootFailure::BootControlWrite)?;
        self.protocol
            .write_blocks(self.media_id, lba, record)
            .map_err(|_| BootFailure::BootControlWrite)?;
        self.protocol
            .flush_blocks()
            .map_err(|_| BootFailure::BootControlWrite)?;
        let mut readback = [0u8; BOOT_CONTROL_RECORD_SIZE];
        self.protocol
            .read_blocks(self.media_id, lba, &mut readback)
            .map_err(|_| BootFailure::BootControlWrite)?;
        if readback != *record {
            return Err(BootFailure::BootControlWrite);
        }
        self.reads = self
            .reads
            .checked_add(u64::try_from(record.len()).map_err(|_| BootFailure::Media)? / sector)
            .ok_or(BootFailure::Media)?;
        self.writes = self
            .writes
            .checked_add(u64::try_from(record.len()).map_err(|_| BootFailure::Media)? / sector)
            .ok_or(BootFailure::Media)?;
        Ok(())
    }
}

struct LoadedBoot {
    boot_info: *mut BootInfo,
    memory_map: *mut MemoryMapEntry,
    kernel_entry: u64,
    manifest_digest: [u8; 32],
    media_reads: u64,
    source_date_epoch: u64,
}

struct VerifiedSlot {
    selection: BootSelection,
    active_control: BootControl,
    manifest: ReleaseManifest,
    manifest_digest: [u8; 32],
    kernel: Component,
    init: Component,
    kernel_payload: Vec<u8>,
    init_payload: Vec<u8>,
}

/// BOOT-001：FDT 头解析纯逻辑（host 可测）——校验 magic（0xd00dfeed）
/// 并读取 `totalsize`（offset 4，大端 u32）。
///
/// 返回 `Some(totalsize)`；magic 不符、长度越界（<8 或 >32MiB）返回 `None`。
fn parse_fdt_header(header: &[u8]) -> Option<u32> {
    if header.len() < 8 {
        return None;
    }
    if header[0] != 0xd0 || header[1] != 0x0d || header[2] != 0xfe || header[3] != 0xed {
        return None;
    }
    let totalsize = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
    if totalsize < 8 || totalsize > 32 * 1024 * 1024 {
        return None;
    }
    Some(totalsize)
}

/// BOOT-001：从 UEFI 配置表定位设备树（DEVICE_TREE_GUID）。
///
/// 返回 `(物理地址, 长度)`；长度从 FDT 头部的 `totalsize` 字段读取。
/// 未找到或 DTB 无效时返回 `None`。
fn locate_device_tree() -> Option<(u64, u64)> {
    // BOOT-001：从 UEFI 配置表定位设备树。
    // 注意：edk2/AAVMF 固件注册的 FDT 配置表 GUID 是 gFdtTableGuid
    // （b1b621d5-f19c-41a5-830b-d9152c69aae0，MdePkg.dec），与 Linux
    // 内核使用的 FDT GUID（...aabe）不同——以固件实际注册值为准。
    const DEVICE_TREE_GUID: uefi::Guid = uefi::guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0");
    uefi::system::with_config_table(|entries| {
        println!("ENEROS stage=config-table-entries count={}", entries.len());
        for entry in entries {
            println!("ENEROS stage=config-table-entry guid={}", entry.guid);
        }
        for entry in entries {
            if entry.guid != DEVICE_TREE_GUID {
                continue;
            }
            let ptr = entry.address as *const u8;
            if ptr.is_null() {
                return None;
            }
            // SAFETY: `entry.address` 来自 UEFI 配置表项，指向有效的 FDT 内存；
            // 先读 8 字节头（`magic`/`totalsize`），长度校验在 `parse_fdt_header` 内完成。
            let header = unsafe { core::slice::from_raw_parts(ptr, 8) };
            let totalsize = parse_fdt_header(header)?;
            return Some((entry.address as u64, totalsize as u64));
        }
        None
    })
}

#[entry]
fn main() -> Status {
    if uefi::helpers::init().is_err() {
        return Status::ABORTED;
    }
    println!("ENEROS stage=uefi-entry profile=qemu-prod secure=required");
    match prepare_boot() {
        // SAFETY: `loaded` 由 `prepare_boot` 完成完整性校验（签名+度量），
        // `handoff` 内部在 `exit_boot_services` 后只访问已分配的 LOADER_DATA 页面。
        Ok(loaded) => unsafe { handoff(loaded) },
        Err(error) => {
            println!(
                "{{\"schema\":\"eneros.boot-rejection.v1\",\"result\":\"REJECT\",\"code\":\"{}\"}}",
                error.code()
            );
            runtime::reset(ResetType::SHUTDOWN, Status::SECURITY_VIOLATION, None)
        }
    }
}

fn prepare_boot() -> Result<LoadedBoot, BootFailure> {
    let handles = boot::locate_handle_buffer(SearchType::from_proto::<BlockIO>())
        .map_err(|_| BootFailure::NoBlockDevice)?;
    for &handle in handles.iter() {
        let Ok(protocol) = boot::open_protocol_exclusive::<BlockIO>(handle) else {
            continue;
        };
        let media = protocol.media();
        if !media.is_media_present()
            || media.is_logical_partition()
            || !matches!(media.block_size(), 512 | 4096)
        {
            continue;
        }
        let Some(total_lbas) = media.last_block().checked_add(1) else {
            continue;
        };
        let sector_size = media.block_size();
        let media_id = media.media_id();
        let mut disk = UefiDisk {
            sector_size,
            total_lbas,
            media_id,
            protocol,
            reads: 0,
            writes: 0,
        };
        let Ok(gpt) = read_gpt(&mut disk) else {
            continue;
        };
        if gpt.disk_guid != EXPECTED_DISK_GUID {
            continue;
        }
        let boot_control_partition = *gpt
            .find_by_name("BOOTCTRL")
            .ok_or(BootFailure::PartitionMissing)?;
        let (first_control, second_control) =
            read_boot_control_copies(&mut disk, &boot_control_partition, gpt.sector_size)?;
        let protected_anchor = read_protected_anchor()?;
        let mut resolved =
            resolve_boot_control(&first_control, &second_control, protected_anchor.as_ref())
                .map_err(|error| {
                    println!("ENEROS stage=bootctrl-resolve-fail error={error:?}");
                    BootFailure::BootControl
                })?;
        if resolved.control.security_floor < MINIMUM_COMPILED_SECURITY_VERSION {
            return Err(BootFailure::BootControl);
        }
        let entropy = read_entropy()?;
        if resolved.needs_anchor_initialization {
            write_protected_anchor(&resolved.anchor)?;
            resolved.needs_anchor_initialization = false;
        }
        let mut verification_fallback = false;
        let verified = loop {
            let (selection, next_control) = resolved
                .control
                .prepare_attempt()
                .map_err(|_| BootFailure::BootControl)?;
            if let Some(next) = next_control {
                let transaction = BootControlTransaction::plan(&resolved, &next)
                    .map_err(|_| BootFailure::BootControl)?;
                persist_boot_control(&mut disk, &boot_control_partition, &transaction)?;
                resolved =
                    refresh_boot_control(&mut disk, &boot_control_partition, gpt.sector_size)?;
            }
            let active_control = resolved.control;
            match verify_slot(&mut disk, &gpt, selection, active_control) {
                Ok(verified) => break verified,
                Err(error)
                    if selection.slot != BootSlot::Recovery
                        && is_slot_verification_failure(error) =>
                {
                    let failed = active_control
                        .mark_verification_failed(selection.slot)
                        .map_err(|_| BootFailure::BootControl)?;
                    let transaction = BootControlTransaction::plan(&resolved, &failed)
                        .map_err(|_| BootFailure::BootControl)?;
                    persist_boot_control(&mut disk, &boot_control_partition, &transaction)?;
                    resolved =
                        refresh_boot_control(&mut disk, &boot_control_partition, gpt.sector_size)?;
                    verification_fallback = true;
                    println!(
                        "ENEROS stage=slot-verification-failed slot={} code={} generation={}",
                        slot_name(selection.slot),
                        error.code(),
                        resolved.control.sequence
                    );
                }
                Err(error) => return Err(error),
            }
        };
        let VerifiedSlot {
            selection,
            active_control,
            manifest,
            manifest_digest: digest,
            kernel,
            init,
            kernel_payload,
            init_payload,
        } = verified;
        // 70.5（WP-70.5.1）：可信身份 capsule 页——BOOT-007 窗口尾页固定
        // 地址（IDENTITY_CAPSULE_PHYS）显式锁定。必须在任何 AnyPages 分配
        // （kernel/init/测量/映射/BootInfo）之前：UEFI 分配器自顶向下，
        // 晚分配会被占页 Address 失败 → OTA 身份缺失。分配失败 = 诚实
        // 降级（soft）：root 读不到 capsule → OTA not-ready，引导继续
        //（不因 OTA 身份面阻断启动）。
        let identity_capsule_page = boot::allocate_pages(
            AllocateType::Address(IDENTITY_CAPSULE_PHYS),
            MemoryType::LOADER_DATA,
            1,
        );
        // BOOT-002：kernel（elfloader）必须用 LOADER_CODE 分配——UEFI 对
        // LOADER_DATA 页设置 NX（不可执行），跳转进入会触发指令预取异常
        // （QEMU 实测 "Synchronous Exception at 0x41000000"）；LOADER_CODE
        // 保留可执行属性。init bundle 为纯数据，维持 LOADER_DATA。
        install_component(&kernel, &kernel_payload, MemoryType::LOADER_CODE)?;
        install_component(&init, &init_payload, MemoryType::LOADER_DATA)?;
        drop(kernel_payload);
        drop(init_payload);

        let boot_nonce = entropy[..32]
            .try_into()
            .map_err(|_| BootFailure::Measurement)?;
        let mut measurements =
            MeasurementLog::new(boot_nonce).map_err(|_| BootFailure::Measurement)?;
        let control_record = active_control
            .encode()
            .map_err(|_| BootFailure::Measurement)?;
        measurements
            .extend(
                MeasurementKind::BootPolicy,
                manifest.header.security_version,
                TRUSTED_KEY_ID,
                boot_policy_digest(active_control.security_floor),
            )
            .and_then(|()| {
                measurements.extend(
                    MeasurementKind::BootControl,
                    active_control.security_floor,
                    selection.image_id,
                    BootControl::digest(&control_record),
                )
            })
            .and_then(|()| {
                measurements.extend(
                    MeasurementKind::Manifest,
                    manifest.header.security_version,
                    manifest.header.release_id,
                    digest,
                )
            })
            .and_then(|()| {
                measurements.extend(
                    MeasurementKind::Kernel,
                    manifest.header.security_version,
                    kernel.content_id,
                    kernel.digest,
                )
            })
            .and_then(|()| {
                measurements.extend(
                    MeasurementKind::InitBundle,
                    manifest.header.security_version,
                    init.content_id,
                    init.digest,
                )
            })
            .map_err(|_| BootFailure::Measurement)?;
        if selection.slot == BootSlot::Recovery {
            measurements
                .extend(
                    MeasurementKind::Recovery,
                    manifest.header.security_version,
                    manifest.header.release_id,
                    digest,
                )
                .map_err(|_| BootFailure::Measurement)?;
        }
        let encoded_measurements = measurements
            .encode()
            .map_err(|_| BootFailure::Measurement)?;
        let measurement_pages = pages_for(MEASUREMENT_LOG_SIZE)?;
        let measurement_allocation = boot::allocate_pages(
            AllocateType::AnyPages,
            MemoryType::LOADER_DATA,
            measurement_pages,
        )
        .map_err(|_| BootFailure::Allocation)?;
        // SAFETY: `measurement_allocation` 由 UEFI `allocate_pages` 返回，
        // 长度为 `measurement_pages * 4096`（checked_mul 防溢出），指针有效。
        let measurement_destination = unsafe {
            slice::from_raw_parts_mut(
                measurement_allocation.as_ptr(),
                measurement_pages
                    .checked_mul(4096)
                    .ok_or(BootFailure::Allocation)?,
            )
        };
        measurement_destination.fill(0);
        measurement_destination[..MEASUREMENT_LOG_SIZE].copy_from_slice(&encoded_measurements);
        let map_pages = pages_for(MAX_MEMORY_MAP_ENTRIES * size_of::<MemoryMapEntry>())?;
        let memory_map =
            boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, map_pages)
                .map_err(|_| BootFailure::Allocation)?
                .as_ptr()
                .cast::<MemoryMapEntry>();
        // SAFETY: `memory_map` 指向 `map_pages` 页的分配（页数按条目数算），
        // 清零范围不越界。
        unsafe {
            memory_map.write_bytes(0, MAX_MEMORY_MAP_ENTRIES);
        }
        let boot_info = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1)
            .map_err(|_| BootFailure::Allocation)?
            .as_ptr()
            .cast::<BootInfo>();
        let reason = if selection.slot == BootSlot::Recovery && verification_fallback {
            BootReason::VerificationFailure
        } else if selection.slot == BootSlot::Recovery
            && active_control.flags & BOOT_CONTROL_FLAG_MANUAL_RECOVERY != 0
        {
            BootReason::ManualRecovery
        } else if selection.slot == BootSlot::Recovery {
            BootReason::Rollback
        } else if selection.trial {
            BootReason::UpdateTrial
        } else {
            BootReason::Cold
        };
        let mut info = BootInfo::new(selection.slot, reason);
        info.flags = BOOT_FLAG_SECURE_BOOT | BOOT_FLAG_ENTROPY_VALID | BOOT_FLAG_MEASUREMENTS_VALID;
        if selection.trial {
            info.flags |= BOOT_FLAG_TRIAL;
        }
        if selection.slot == BootSlot::Recovery {
            info.flags |= BOOT_FLAG_RECOVERY;
        }
        info.boot_id.copy_from_slice(&entropy[..16]);
        info.kernel_entry = kernel.entry_address;
        info.init_bundle_phys = init.load_address;
        info.init_bundle_len = init.media_length;
        info.entropy_seed = entropy;
        info.manifest_digest = digest;
        info.measurement_log_phys = measurement_allocation.as_ptr() as u64;
        info.measurement_log_len = MEASUREMENT_LOG_SIZE as u64;
        info.bootctrl_generation = active_control.sequence;
        info.security_version = manifest.header.security_version;
        // BOOT-001：从 UEFI 配置表注入设备树（DTB）——旧实现 dtb_phys/dtb_len
        // 恒为 0，内核无设备树（无法初始化真实外设）。QEMU virt / 飞腾 D2000
        // 的 UEFI 均通过 DEVICE_TREE_GUID 配置表项暴露 DTB。
        let dtb = locate_device_tree();
        match dtb {
            Some((phys, len)) => {
                // WP-27.2 诊断：DTB 定位实况（失败时定位检查点）。
                println!(
                    "ENEROS stage=dtb-locate phys=0x{:x} len={} max={} init_media_length={}",
                    phys, len, ENEROS_DTB_MAX_LEN, init.media_length
                );
                // WP-27.2（蓝图/27 §6）：把运行时 DTB 拷贝到内核 BOOT-007
                // 映射窗口内 bundle 之后的动态位置（dtb_window_for），
                // root-task 以相同推导直接读物理地址 → parse_fdt → 驱动绑定。
                // 约束（fail-closed，超限即引导失败，不静默截断）：
                //   1) bundle header total_len 必须与验签的 init.media_length
                //      一致（否则 root-task 推导的 DTB 位置漂移）；
                //   2) len ≤ ENEROS_DTB_MAX_LEN；
                //   3) len ≤ 窗口剩余空间（dtb_window_for）。
                let mut bundle_header = [0u8; 16];
                // SAFETY: `ENEROS_INIT_BUNDLE_PHYS` 处为 install_component
                // 刚写入的 init bundle（LOADER_DATA 分配），16 字节头可读。
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        ENEROS_INIT_BUNDLE_PHYS as *const u8,
                        bundle_header.as_mut_ptr(),
                        16,
                    );
                }
                let header_total =
                    u32::from_le_bytes(bundle_header[12..16].try_into().unwrap_or([0; 4])) as u64;
                if header_total != init.media_length {
                    return Err(BootFailure::DtLayout);
                }
                let Some((dtb_phys, window)) = dtb_window_for(init.media_length) else {
                    return Err(BootFailure::DtLayout);
                };
                if len > ENEROS_DTB_MAX_LEN || len > window {
                    return Err(BootFailure::DtLayout);
                }
                // SAFETY: `phys` 来自 UEFI 配置表项（firmware 拥有的有效
                // 内存，boot services 阶段可读）；`dtb_phys` 为 BOOT-007
                // 窗口内普通 RAM（未分配 untyped，见 BOOT-007 预留），
                // 长度已受 `ENEROS_DTB_MAX_LEN` 与窗口剩余空间双重约束；
                // 与 bundle 不重叠（页对齐起点在 bundle 末端之后）。拷贝
                // 发生在 exit_boot_services 之前，UEFI 内存仍有效。
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        phys as *const u8,
                        dtb_phys as *mut u8,
                        len as usize,
                    );
                }
                info.dtb_phys = dtb_phys;
                info.dtb_len = len;
            }
            None => {
                // 无 DTB 时内核无法初始化外设——视为启动失败（fail-closed）。
                return Err(BootFailure::NoDeviceTree);
            }
        }
        // SAFETY: `boot_info` 指向单页 LOADER_DATA 分配，`BootInfo` 大小
        // 不超过一页；写入为独占所有权（prepare_boot 内不再使用该指针）。
        unsafe {
            boot_info.write(info);
        }
        // 70.5（WP-70.5.1/70.5.4）：可信身份 capsule 写入——验签链的只读
        // 投影（ReleaseManifest + anchor 匹配 BootControl），root-task 经
        // 固定物理地址消费并分发 svc-ota。写失败 = OTA not-ready（诚实
        // 降级，不阻断引导）。
        match identity_capsule_page {
            Ok(page) => {
                let mut capsule = IdentityCapsule::new(
                    selection.slot as u8,
                    info.boot_id,
                    manifest.header.release_id,
                    manifest.header.platform_id,
                    digest,
                    active_control.sequence,
                    manifest.header.security_version,
                    active_control.security_floor,
                    [
                        manifest.header.product_version.major,
                        manifest.header.product_version.minor,
                        manifest.header.product_version.patch,
                    ],
                );
                capsule.seal();
                let bytes = capsule.to_bytes();
                // SAFETY: `page` 为本函数以 Address 锁定的单页 LOADER_DATA
                //（exit_boot_services 后保留）；写入范围 = capsule 编码
                // 长度（136B < 4 KiB 页）。
                let dst = page.as_ptr() as *mut u8;
                unsafe {
                    core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
                }
                println!(
                    "ENEROS stage=identity-capsule slot={} gen={} secver={} floor={} ver={}.{}.{}",
                    slot_name(selection.slot),
                    active_control.sequence,
                    manifest.header.security_version,
                    active_control.security_floor,
                    manifest.header.product_version.major,
                    manifest.header.product_version.minor,
                    manifest.header.product_version.patch,
                );
            }
            Err(_) => {
                println!("ENEROS stage=identity-capsule-alloc-fail — OTA not-ready (soft degrade)");
            }
        }
        println!(
            "ENEROS stage=verified slot={} security_version={} bootctrl_generation={} media_reads={} media_writes={} kernel_bytes={} init_bytes={}",
            slot_name(selection.slot),
            manifest.header.security_version,
            active_control.sequence,
            disk.reads,
            disk.writes,
            kernel.media_length,
            init.media_length
        );
        // 42：正式介质黑盒启动的结构化 boot-evidence。必须在
        // exit_boot_services 之前打印（之后 UEFI console boot service 不可用）。
        let memory_map_count = boot::memory_map(MemoryType::LOADER_DATA)
            .map(|map| map.entries().count())
            .unwrap_or(0);
        println!(
            "{{\"schema\":\"eneros.boot-evidence.v1\",\"result\":\"PASS\",\"slot\":\"{}\",\"boot_reason\":\"{}\",\"security_version\":{},\"bootctrl_generation\":{},\"memory_map_count\":{},\"init_bytes\":{},\"source_date_epoch\":{},\"manifest_sha256\":\"{}\",\"measurement_root_sha256\":\"{}\",\"measurement_log_hex\":\"{}\"}}",
            slot_name(selection.slot),
            boot_reason_name(reason),
            manifest.header.security_version,
            active_control.sequence,
            memory_map_count,
            init.media_length,
            manifest.header.source_date_epoch,
            eneros_hex::encode_lower(&digest),
            eneros_hex::encode_lower(&encoded_measurements[64..96]),
            eneros_hex::encode_lower(&encoded_measurements)
        );
        let source_date_epoch = manifest.header.source_date_epoch;
        drop(manifest);
        let media_reads = disk.reads;
        drop(disk);
        drop(handles);
        return Ok(LoadedBoot {
            boot_info,
            memory_map,
            kernel_entry: kernel.entry_address,
            manifest_digest: digest,
            media_reads,
            source_date_epoch,
        });
    }
    Err(BootFailure::DiskIdentity)
}

fn verify_slot(
    disk: &mut UefiDisk,
    gpt: &Gpt,
    selection: BootSelection,
    active_control: BootControl,
) -> Result<VerifiedSlot, BootFailure> {
    let (partition_name, manifest_slot) = match selection.slot {
        BootSlot::A => ("BOOT_A", ManifestSlot::A),
        BootSlot::B => ("BOOT_B", ManifestSlot::B),
        BootSlot::Recovery => ("RECOVERY", ManifestSlot::Recovery),
    };
    let partition = *gpt
        .find_by_name(partition_name)
        .ok_or(BootFailure::PartitionMissing)?;
    let partition_length = partition
        .last_lba
        .checked_sub(partition.first_lba)
        .and_then(|value| value.checked_add(1))
        .and_then(|value| value.checked_mul(u64::from(disk.sector_size())))
        .ok_or(BootFailure::Media)?;
    let mut reader =
        PartitionReader::new(disk, &partition).map_err(|_| BootFailure::PartitionRead)?;
    let mut scratch = vec![0u8; usize::try_from(gpt.sector_size).map_err(|_| BootFailure::Media)?];
    let mut header = [0u8; MANIFEST_HEADER_LEN];
    reader
        .read_exact(MANIFEST_MEDIA_OFFSET, &mut header, &mut scratch)
        .map_err(|_| BootFailure::PartitionRead)?;
    let manifest_len = declared_manifest_len(&header).map_err(BootFailure::Manifest)?;
    let mut encoded_manifest = vec![0u8; manifest_len];
    reader
        .read_exact(MANIFEST_MEDIA_OFFSET, &mut encoded_manifest, &mut scratch)
        .map_err(|_| BootFailure::PartitionRead)?;
    let policy = TrustPolicy {
        platform_id: PLATFORM_ID,
        key_id: TRUSTED_KEY_ID,
        public_key: TRUSTED_PUBLIC_KEY,
        minimum_security_version: active_control.security_floor,
        expected_security_version: selection.security_version,
        expected_release_id: selection.image_id,
        selected_slot: manifest_slot,
        partition_length,
    };
    let manifest = verify_manifest(&encoded_manifest, &policy).map_err(BootFailure::Manifest)?;
    let digest = manifest_digest(&encoded_manifest);
    let kernel = manifest
        .components
        .iter()
        .find(|component| component.role == ComponentRole::Kernel)
        .ok_or(BootFailure::ComponentMissing)?
        .clone();
    let init = manifest
        .components
        .iter()
        .find(|component| component.role == ComponentRole::InitBundle)
        .ok_or(BootFailure::ComponentMissing)?
        .clone();
    let kernel_payload = read_verified_component(&mut reader, &kernel, &mut scratch)?;
    let init_payload = read_verified_component(&mut reader, &init, &mut scratch)?;
    Ok(VerifiedSlot {
        selection,
        active_control,
        manifest,
        manifest_digest: digest,
        kernel,
        init,
        kernel_payload,
        init_payload,
    })
}

const fn is_slot_verification_failure(error: BootFailure) -> bool {
    matches!(
        error,
        BootFailure::PartitionRead | BootFailure::Manifest(_) | BootFailure::ComponentMissing
    )
}

fn refresh_boot_control(
    disk: &mut UefiDisk,
    partition: &ParsedPartition,
    sector_size: u32,
) -> Result<ResolvedBootControl, BootFailure> {
    let (first, second) = read_boot_control_copies(disk, partition, sector_size)?;
    let protected = read_protected_anchor()?.ok_or(BootFailure::ProtectedState)?;
    let resolved = resolve_boot_control(&first, &second, Some(&protected))
        .map_err(|_| BootFailure::BootControl)?;
    if resolved.control.security_floor < MINIMUM_COMPILED_SECURITY_VERSION {
        return Err(BootFailure::BootControl);
    }
    Ok(resolved)
}

fn read_boot_control_copies(
    disk: &mut UefiDisk,
    partition: &ParsedPartition,
    sector_size: u32,
) -> Result<
    (
        [u8; BOOT_CONTROL_RECORD_SIZE],
        [u8; BOOT_CONTROL_RECORD_SIZE],
    ),
    BootFailure,
> {
    let mut reader = PartitionReader::new(disk, partition).map_err(|_| BootFailure::BootControl)?;
    let mut scratch = vec![0u8; usize::try_from(sector_size).map_err(|_| BootFailure::Media)?];
    let mut first = [0u8; BOOT_CONTROL_RECORD_SIZE];
    let mut second = [0u8; BOOT_CONTROL_RECORD_SIZE];
    reader
        .read_exact(BOOT_CONTROL_COPY_OFFSETS[0], &mut first, &mut scratch)
        .map_err(|_| BootFailure::BootControl)?;
    reader
        .read_exact(BOOT_CONTROL_COPY_OFFSETS[1], &mut second, &mut scratch)
        .map_err(|_| BootFailure::BootControl)?;
    Ok((first, second))
}

fn read_protected_anchor() -> Result<Option<BootControlAnchor>, BootFailure> {
    let mut bytes = [0u8; BOOT_CONTROL_ANCHOR_SIZE];
    match runtime::get_variable(
        uefi::cstr16!("EnerOSBootControlAnchor"),
        &BOOT_CONTROL_VENDOR,
        &mut bytes,
    ) {
        Ok((value, attributes)) => {
            let expected =
                VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS;
            if attributes != expected {
                return Err(BootFailure::ProtectedState);
            }
            BootControlAnchor::decode(value)
                .map(Some)
                .map_err(|_| BootFailure::ProtectedState)
        }
        Err(error) if error.status() == Status::NOT_FOUND => Ok(None),
        Err(_) => Err(BootFailure::ProtectedState),
    }
}

fn write_protected_anchor(anchor: &BootControlAnchor) -> Result<(), BootFailure> {
    let bytes = anchor.encode().map_err(|_| BootFailure::ProtectedState)?;
    let attributes = VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS;
    runtime::set_variable(
        uefi::cstr16!("EnerOSBootControlAnchor"),
        &BOOT_CONTROL_VENDOR,
        attributes,
        &bytes,
    )
    .map_err(|_| BootFailure::ProtectedState)?;
    if read_protected_anchor()? != Some(*anchor) {
        return Err(BootFailure::ProtectedState);
    }
    Ok(())
}

fn persist_boot_control(
    disk: &mut UefiDisk,
    partition: &ParsedPartition,
    transaction: &BootControlTransaction,
) -> Result<(), BootFailure> {
    disk.write_control_copy(partition, transaction.target.offset(), &transaction.record)?;
    println!(
        "ENEROS stage=bootctrl-target-written generation={}",
        transaction.anchor.sequence
    );
    write_protected_anchor(&transaction.anchor)?;
    println!(
        "ENEROS stage=bootctrl-anchor-committed generation={}",
        transaction.anchor.sequence
    );
    disk.write_control_copy(partition, transaction.mirror.offset(), &transaction.record)?;
    println!(
        "ENEROS stage=bootctrl-mirror-written generation={}",
        transaction.anchor.sequence
    );
    Ok(())
}

const fn slot_name(slot: BootSlot) -> &'static str {
    match slot {
        BootSlot::A => "A",
        BootSlot::B => "B",
        BootSlot::Recovery => "RECOVERY",
    }
}

fn read_verified_component<D: BlockDevice>(
    reader: &mut PartitionReader<'_, D>,
    component: &Component,
    scratch: &mut [u8],
) -> Result<Vec<u8>, BootFailure> {
    let media_length =
        usize::try_from(component.media_length).map_err(|_| BootFailure::Allocation)?;
    let mut payload = vec![0u8; media_length];
    reader
        .read_exact(component.media_offset, &mut payload, scratch)
        .map_err(|_| BootFailure::PartitionRead)?;
    verify_component(component, &payload).map_err(BootFailure::Manifest)?;
    Ok(payload)
}

fn install_component(
    component: &Component,
    payload: &[u8],
    memory_type: MemoryType,
) -> Result<(), BootFailure> {
    verify_component(component, payload).map_err(BootFailure::Manifest)?;
    let memory_length =
        usize::try_from(component.memory_length).map_err(|_| BootFailure::Allocation)?;
    let pages = pages_for(memory_length)?;
    let allocation = boot::allocate_pages(
        AllocateType::Address(component.load_address),
        memory_type,
        pages,
    )
    .map_err(|_| BootFailure::Allocation)?;
    // SAFETY: `allocation` 由 `allocate_pages` 分配 `pages` 页，长度合法。
    let destination = unsafe { slice::from_raw_parts_mut(allocation.as_ptr(), pages * 4096) };
    destination.fill(0);
    let destination = destination
        .get_mut(..payload.len())
        .ok_or(BootFailure::Allocation)?;
    destination.copy_from_slice(payload);
    Ok(())
}

fn read_entropy() -> Result<[u8; 64], BootFailure> {
    let handles = boot::locate_handle_buffer(SearchType::from_proto::<Rng>())
        .map_err(|_| BootFailure::EntropyUnavailable)?;
    let mut protocol_seen = false;
    for &handle in handles.iter() {
        let Ok(mut rng) = boot::open_protocol_exclusive::<Rng>(handle) else {
            continue;
        };
        protocol_seen = true;
        let mut raw = [0u8; RAW_BOOT_ENTROPY_SIZE];
        if rng.get_rng(None, &mut raw).is_ok() {
            let conditioned = condition_boot_entropy(&raw);
            volatile_clear(&mut raw);
            if let Ok(seed) = conditioned {
                return Ok(seed);
            }
        } else {
            volatile_clear(&mut raw);
        }
    }
    if protocol_seen {
        Err(BootFailure::EntropyFailure)
    } else {
        Err(BootFailure::EntropyUnavailable)
    }
}

fn volatile_clear(bytes: &mut [u8]) {
    for byte in bytes {
        // SAFETY: `byte` 来自 `&mut [u8]`，指向有效可写内存；volatile 写
        // 防止编译器消除清零（敏感数据擦除语义）。
        unsafe {
            core::ptr::write_volatile(byte, 0);
        }
    }
}

fn boot_policy_digest(security_floor: u64) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"EnerOS/boot-policy/v1");
    hash.update(PLATFORM_ID);
    hash.update(TRUSTED_KEY_ID);
    hash.update(TRUSTED_PUBLIC_KEY);
    hash.update(security_floor.to_le_bytes());
    hash.finalize().into()
}

fn pages_for(length: usize) -> Result<usize, BootFailure> {
    length
        .checked_add(4095)
        .map(|value| value / 4096)
        .filter(|pages| *pages != 0)
        .ok_or(BootFailure::Allocation)
}

fn boot_reason_name(reason: BootReason) -> &'static str {
    match reason {
        BootReason::Cold => "COLD",
        BootReason::Watchdog => "WATCHDOG",
        BootReason::UpdateTrial => "UPDATE_TRIAL",
        BootReason::Rollback => "ROLLBACK",
        BootReason::ManualRecovery => "MANUAL_RECOVERY",
        BootReason::VerificationFailure => "VERIFICATION_FAILURE",
    }
}

/// 退出启动服务并跳转内核。
///
/// # Safety
/// 必须在所有 UEFI 服务不再被使用后调用；`loaded` 的分配必须在
/// `exit_boot_services` 保留区域（LOADER_DATA）内。
unsafe fn handoff(loaded: LoadedBoot) -> ! {
    // SAFETY: 调用方保证所有 UEFI 服务调用已完成；`exit_boot_services`
    // 之后仅能使用内存映射中的保留区域。
    let memory_map = unsafe { boot::exit_boot_services(Some(MemoryType::LOADER_DATA)) };
    let mut count = 0usize;
    for descriptor in memory_map.entries() {
        if count == MAX_MEMORY_MAP_ENTRIES {
            shutdown_after_exit();
        }
        let length = match descriptor.page_count.checked_mul(4096) {
            Some(length) if length != 0 => length,
            _ => shutdown_after_exit(),
        };
        let kind = if descriptor.ty == MemoryType::CONVENTIONAL {
            MemoryRegionKind::Usable
        } else if descriptor.ty == MemoryType::MMIO || descriptor.ty == MemoryType::MMIO_PORT_SPACE
        {
            MemoryRegionKind::Mmio
        } else if descriptor.ty == MemoryType::PERSISTENT_MEMORY {
            MemoryRegionKind::Persistent
        } else {
            MemoryRegionKind::Firmware
        };
        // SAFETY: `count < MAX_MEMORY_MAP_ENTRIES`（上方已检查），
        // `loaded.memory_map` 为按该上限分配的页面，写入不越界。
        unsafe {
            loaded.memory_map.add(count).write(MemoryMapEntry {
                base: descriptor.phys_start,
                length,
                kind,
                attributes: 0,
                reserved: 0,
            });
        }
        count += 1;
    }
    // SAFETY: `loaded.boot_info` 指向 prepare_boot 分配的 BootInfo 页，
    // 且在 `exit_boot_services` 保留区域，地址仍有效。
    let info = unsafe { &mut *loaded.boot_info };
    info.memory_map_phys = loaded.memory_map as u64;
    info.memory_map_count = u32::try_from(count).unwrap_or(0);
    info.memory_map_len = u64::try_from(count * size_of::<MemoryMapEntry>()).unwrap_or(0);
    info.seal();
    if info.validate().is_err() {
        shutdown_after_exit();
    }
    let _ = (loaded.manifest_digest, loaded.media_reads);
    forget(memory_map);
    // SAFETY: `loaded.kernel_entry` 为已验签内核 ELF 的入口物理地址，
    // 由 boot-info 解析验证；转换为相同 ABI 的函数指针（UB 风险已由
    // 签名验证环节消除——仅信任已验证镜像的入口）。
    let entry: extern "C" fn(*const BootInfo) -> ! = unsafe {
        core::mem::transmute::<usize, extern "C" fn(*const BootInfo) -> !>(
            loaded.kernel_entry as usize,
        )
    };
    entry(loaded.boot_info)
}

fn shutdown_after_exit() -> ! {
    let function = 0x8400_0008u64;
    // SAFETY: `hvc #0` 触发 PSCI SYSTEM_OFF（x0 = 0x8400_0008），
    // 无内存访问、无返回（noreturn），不破坏任何状态。
    unsafe { core::arch::asm!("hvc #0", in("x0") function, options(noreturn)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BOOT-001：FDT magic + totalsize 解析。
    #[test]
    fn fdt_header_valid() {
        let mut h = [0u8; 8];
        h[0] = 0xd0;
        h[1] = 0x0d;
        h[2] = 0xfe;
        h[3] = 0xed;
        h[4] = 0x00;
        h[5] = 0x00;
        h[6] = 0x10;
        h[7] = 0x00; // totalsize = 0x1000
        assert_eq!(parse_fdt_header(&h), Some(0x1000));
    }

    /// BOOT-001：错误 magic → None。
    #[test]
    fn fdt_header_bad_magic() {
        let h = [0x00u8; 8];
        assert_eq!(parse_fdt_header(&h), None);
    }

    /// BOOT-001：totalsize 越界（<8 / >32MiB）→ None。
    #[test]
    fn fdt_header_size_bounds() {
        let mut tiny = [0u8; 8];
        tiny[0] = 0xd0;
        tiny[1] = 0x0d;
        tiny[2] = 0xfe;
        tiny[3] = 0xed;
        assert_eq!(parse_fdt_header(&tiny), None); // totalsize = 0 < 8

        let mut huge = [0u8; 8];
        huge[0] = 0xd0;
        huge[1] = 0x0d;
        huge[2] = 0xfe;
        huge[3] = 0xed;
        huge[4] = 0x02; // 0x0200_0000 = 32MiB + 1 区域起点
        assert_eq!(parse_fdt_header(&huge), None);
    }

    /// BOOT-001：截断头部（<8 字节）→ None。
    #[test]
    fn fdt_header_truncated() {
        assert_eq!(parse_fdt_header(&[0xd0, 0x0d, 0xfe]), None);
    }
}
