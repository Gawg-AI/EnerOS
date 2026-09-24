use core::mem::{align_of, size_of};

pub const BOOT_INFO_MAGIC: [u8; 8] = *b"ENRBOOT\0";
pub const BOOT_INFO_ABI_MAJOR: u16 = 1;
pub const BOOT_INFO_ABI_MINOR: u16 = 0;
pub const BOOT_INFO_SIZE: usize = 256;

pub const BOOT_FLAG_SECURE_BOOT: u64 = 1 << 0;
pub const BOOT_FLAG_RECOVERY: u64 = 1 << 1;
pub const BOOT_FLAG_TRIAL: u64 = 1 << 2;
pub const BOOT_FLAG_ENTROPY_VALID: u64 = 1 << 3;
pub const BOOT_FLAG_MEASUREMENTS_VALID: u64 = 1 << 4;
pub const BOOT_INFO_KNOWN_FLAGS: u64 = BOOT_FLAG_SECURE_BOOT
    | BOOT_FLAG_RECOVERY
    | BOOT_FLAG_TRIAL
    | BOOT_FLAG_ENTROPY_VALID
    | BOOT_FLAG_MEASUREMENTS_VALID;

const CRC_OFFSET: usize = 240;
/// 30 RT-022：init bundle 上限单一事实源——与内核 BOOT-007 映射窗口对齐
/// （ENEROS_MAPPED_WINDOW_LEN，2 MiB）。旧值 256 MiB 与 root-task 64 MiB 与
/// 内核窗口 2 MiB 三重不一致（契约违背：bundle 超窗即越窗读 VM fault）。
/// bundle >2 MiB 需求须先扩内核窗口（R30-1，登记）。
pub const ENEROS_INIT_BUNDLE_MAX_LEN: u64 = ENEROS_MAPPED_WINDOW_LEN;
const MAX_DTB_LEN: u64 = 16 * 1024 * 1024;
const MAX_MEMORY_MAP_LEN: u64 = 1024 * 1024;
const MAX_MEASUREMENT_LOG_LEN: u64 = 1024 * 1024;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootSlot {
    A = 1,
    B = 2,
    Recovery = 3,
}

impl TryFrom<u8> for BootSlot {
    type Error = BootInfoError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::A),
            2 => Ok(Self::B),
            3 => Ok(Self::Recovery),
            _ => Err(BootInfoError::InvalidSlot),
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootReason {
    Cold = 1,
    Watchdog = 2,
    UpdateTrial = 3,
    Rollback = 4,
    ManualRecovery = 5,
    VerificationFailure = 6,
}

impl TryFrom<u8> for BootReason {
    type Error = BootInfoError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Cold),
            2 => Ok(Self::Watchdog),
            3 => Ok(Self::UpdateTrial),
            4 => Ok(Self::Rollback),
            5 => Ok(Self::ManualRecovery),
            6 => Ok(Self::VerificationFailure),
            _ => Err(BootInfoError::InvalidReason),
        }
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRegionKind {
    Usable = 1,
    Reserved = 2,
    Firmware = 3,
    Mmio = 4,
    Persistent = 5,
}

impl TryFrom<u32> for MemoryRegionKind {
    type Error = BootInfoError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Usable),
            2 => Ok(Self::Reserved),
            3 => Ok(Self::Firmware),
            4 => Ok(Self::Mmio),
            5 => Ok(Self::Persistent),
            _ => Err(BootInfoError::MemoryMap),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryMapEntry {
    pub base: u64,
    pub length: u64,
    pub kind: MemoryRegionKind,
    pub attributes: u32,
    pub reserved: u64,
}

impl MemoryMapEntry {
    pub const ENCODED_SIZE: usize = 32;

    pub fn validate(&self) -> Result<(), BootInfoError> {
        if self.length == 0 || self.base.checked_add(self.length).is_none() {
            return Err(BootInfoError::Range);
        }
        if self.reserved != 0 {
            return Err(BootInfoError::Reserved);
        }
        Ok(())
    }

    pub fn to_le_bytes(&self) -> Result<[u8; Self::ENCODED_SIZE], BootInfoError> {
        self.validate()?;
        let mut bytes = [0u8; Self::ENCODED_SIZE];
        put_u64(&mut bytes, 0, self.base);
        put_u64(&mut bytes, 8, self.length);
        put_u32(&mut bytes, 16, self.kind as u32);
        put_u32(&mut bytes, 20, self.attributes);
        put_u64(&mut bytes, 24, self.reserved);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BootInfoError> {
        if bytes.len() != Self::ENCODED_SIZE {
            return Err(BootInfoError::Length);
        }
        let entry = Self {
            base: get_u64(bytes, 0),
            length: get_u64(bytes, 8),
            kind: MemoryRegionKind::try_from(get_u32(bytes, 16))?,
            attributes: get_u32(bytes, 20),
            reserved: get_u64(bytes, 24),
        };
        entry.validate()?;
        if entry.to_le_bytes()?.as_slice() != bytes {
            return Err(BootInfoError::MemoryMap);
        }
        Ok(entry)
    }
}

pub fn validate_memory_map_bytes(bytes: &[u8], count: u32) -> Result<(), BootInfoError> {
    let count = usize::try_from(count).map_err(|_| BootInfoError::MemoryMap)?;
    if count == 0
        || count
            .checked_mul(MemoryMapEntry::ENCODED_SIZE)
            .ok_or(BootInfoError::MemoryMap)?
            != bytes.len()
        || bytes.len() > MAX_MEMORY_MAP_LEN as usize
    {
        return Err(BootInfoError::MemoryMap);
    }
    for index in 0..count {
        let offset = index * MemoryMapEntry::ENCODED_SIZE;
        let current =
            MemoryMapEntry::decode(&bytes[offset..offset + MemoryMapEntry::ENCODED_SIZE])?;
        let current_end = current
            .base
            .checked_add(current.length)
            .ok_or(BootInfoError::Range)?;
        for previous_index in 0..index {
            let previous_offset = previous_index * MemoryMapEntry::ENCODED_SIZE;
            let previous = MemoryMapEntry::decode(
                &bytes[previous_offset..previous_offset + MemoryMapEntry::ENCODED_SIZE],
            )?;
            let previous_end = previous
                .base
                .checked_add(previous.length)
                .ok_or(BootInfoError::Range)?;
            if current.base < previous_end && previous.base < current_end {
                return Err(BootInfoError::MemoryMap);
            }
        }
    }
    Ok(())
}

/// WP-27.2（蓝图/27 §6 装配链）：boot-manager 把运行时 DTB 拷贝到内核
/// BOOT-007 映射窗口内（`third_party/sel4-patches/0001-init-bundle-mapping.patch`，
/// 2 MiB 1:1 映射进初始线程 VSpace），root-task 与 init bundle 同模式直接读
/// 物理地址。DTB 源为 UEFI 配置表（gFdtTableGuid，boot-manager
/// `locate_device_tree`），拷贝保证 root-task 拿到**运行时真实 DTB**（非
/// 构建期快照）。DTB 起点为**动态**（bundle 之后页对齐，见
/// [`dtb_window_for`]）——QEMU virt 实测运行时 DTB ≈ 1 MiB + 4 KiB，固定
/// 起点方案对 DTB 大小脆弱（WP-27.2 QEMU 实测 NO_DEVICE_TREE 后改进）。
pub const ENEROS_INIT_BUNDLE_PHYS: u64 = 0x4200_0000;
/// BOOT-007 内核映射窗口长度（4 MiB，kernel boot.c 常量；39 收尾复验
/// 由 2 MiB 扩容——bundle 随 EL0 服务数量/尺寸增长，2 MiB 窗口的 DTB
/// 容纳上限被超出（DT_LAYOUT 拒绝））。
/// 49 收口：4 MiB→8 MiB（远程 dual-brain 契约/传输面使 bundle 增至
/// 3.24 MiB，4 MiB 窗内 DTB 容纳上限被击穿——DT_LAYOUT fail-closed 实弹
/// 拦截；39 卷 2→4 MiB 同款先例。保留区 32 MiB（overlay dts）内扩窗）。
pub const ENEROS_MAPPED_WINDOW_LEN: u64 = 0x800_000;
/// BOOT-007 **保留窗全长**（单一事实源 =
/// `third_party/sel4-top/overlay-init-bundle-reserved.dts` 的
/// `/reserved-memory/eneros-init-bundle@42000000` reg 长度 0x02000000）。
/// 内核只 1:1 映射前 [`ENEROS_MAPPED_WINDOW_LEN`]，其余 24 MiB 是「保留
/// 不映射」的余量（扩窗历史 2→4→8 MiB 均在保留区内就地扩）。
///
/// root-task 用它把 bootinfo 中与保留窗重叠的 untyped 从分配器**拉黑**：
/// seL4 上游 `create_untypeds` 会把 normal 内存区间之间的洞按 device
/// untyped 发放（overlay 生效时保留窗 = avail 两段之间的洞 → 32 MiB
/// device untyped 实证见 evidence/70/70.6 日志 DEV-UNTYPED[21]；overlay
/// 缺失时该窗混入 normal untyped）。窗口内 bundle 模块 sha256 表与
/// blob 同窗自参照、运行时 DTB 仅 FDT magic 自检、身份 capsule 仅
/// CRC32——任何经 retype/映射拿到的窗内写权都不可检出，必须 fail-closed
/// 在分配入口拒绝（2026-09-23 审计复核施工）。
pub const ENEROS_RESERVED_WINDOW_LEN: u64 = 0x200_0000;
/// 单 DTB 硬上限（**独立防御上限，非窗口全尺寸**——WP-74.2.3 措辞订正：
/// 映射窗已 8 MiB（[`ENEROS_MAPPED_WINDOW_LEN`]），本常量 2 MiB 与之无关；
/// QEMU virt 运行时 DTB ≈ 0x101000，真机平台 dts 一般更小）。实际边界由
/// [`dtb_window_for`] 的窗口剩余空间决定（bundle 已占部分之外）；超限
/// fail-closed。
pub const ENEROS_DTB_MAX_LEN: u64 = 0x200_000;

/// bundle 之后的 DTB 放置窗口：起点 = bundle 末端页对齐，长度 = 窗口内
/// 剩余空间。bundle 超出窗口 → `None`（fail-closed）。
///
/// 调用方（boot-manager 拷贝处 / root-task 读取处）必须用**同一 bundle
/// 长度**推导，否则 DTB 位置漂移——boot-manager 用验签后的
/// `init.media_length` 并交叉校验 bundle header `total_len` 一致；
/// root-task 用已解析的 bundle header `total_len`。
///
/// 70.5（WP-70.5.1）：窗口尾页为可信身份 capsule 预留
/// （`IDENTITY_CAPSULE_RESERVE`）——DTB 永不触及 capsule 页，两端经本
/// 函数单源推导自动一致。
pub const fn dtb_window_for(bundle_len: u64) -> Option<(u64, u64)> {
    let start = (ENEROS_INIT_BUNDLE_PHYS + bundle_len + 0xFFF) & !0xFFF;
    let end = ENEROS_INIT_BUNDLE_PHYS + ENEROS_MAPPED_WINDOW_LEN
        - crate::identity::IDENTITY_CAPSULE_RESERVE;
    if start >= end {
        return None;
    }
    Some((start, end - start))
}

#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootInfo {
    pub magic: [u8; 8],
    pub abi_major: u16,
    pub abi_minor: u16,
    pub struct_size: u32,
    pub flags: u64,
    pub boot_id: [u8; 16],
    pub selected_slot: BootSlot,
    pub boot_reason: BootReason,
    pub reserved0: [u8; 6],
    pub kernel_entry: u64,
    pub init_bundle_phys: u64,
    pub init_bundle_len: u64,
    pub dtb_phys: u64,
    pub dtb_len: u64,
    pub memory_map_phys: u64,
    pub memory_map_len: u64,
    pub memory_map_count: u32,
    pub memory_map_entry_size: u32,
    pub measurement_log_phys: u64,
    pub measurement_log_len: u64,
    pub entropy_seed: [u8; 64],
    pub manifest_digest: [u8; 32],
    pub bootctrl_generation: u64,
    pub security_version: u64,
    pub crc32: u32,
    pub reserved1: [u8; 12],
}

const _: () = assert!(size_of::<BootInfo>() == BOOT_INFO_SIZE);
const _: () = assert!(align_of::<BootInfo>() == 16);
const _: () = assert!(size_of::<MemoryMapEntry>() == MemoryMapEntry::ENCODED_SIZE);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootInfoError {
    Length,
    Magic,
    Version,
    StructSize,
    Flags,
    InvalidSlot,
    InvalidReason,
    Reserved,
    Crc,
    Range,
    Alignment,
    MissingEntropy,
    MissingManifestDigest,
    MemoryMap,
    MeasurementLog,
    /// WP-37.6：bundle 与 DTB 区间重叠（BOOT-007 窗口内两个负载不能叠放）。
    Overlap,
}

impl BootInfo {
    pub const fn new(selected_slot: BootSlot, boot_reason: BootReason) -> Self {
        Self {
            magic: BOOT_INFO_MAGIC,
            abi_major: BOOT_INFO_ABI_MAJOR,
            abi_minor: BOOT_INFO_ABI_MINOR,
            struct_size: BOOT_INFO_SIZE as u32,
            flags: 0,
            boot_id: [0; 16],
            selected_slot,
            boot_reason,
            reserved0: [0; 6],
            kernel_entry: 0,
            init_bundle_phys: 0,
            init_bundle_len: 0,
            dtb_phys: 0,
            dtb_len: 0,
            memory_map_phys: 0,
            memory_map_len: 0,
            memory_map_count: 0,
            memory_map_entry_size: MemoryMapEntry::ENCODED_SIZE as u32,
            measurement_log_phys: 0,
            measurement_log_len: 0,
            entropy_seed: [0; 64],
            manifest_digest: [0; 32],
            bootctrl_generation: 0,
            security_version: 0,
            crc32: 0,
            reserved1: [0; 12],
        }
    }

    pub fn seal(&mut self) {
        self.crc32 = 0;
        self.crc32 = crc32(&self.to_le_bytes());
    }

    pub fn validate(&self) -> Result<(), BootInfoError> {
        if self.magic != BOOT_INFO_MAGIC {
            return Err(BootInfoError::Magic);
        }
        if self.abi_major != BOOT_INFO_ABI_MAJOR || self.abi_minor > BOOT_INFO_ABI_MINOR {
            return Err(BootInfoError::Version);
        }
        if self.struct_size as usize != BOOT_INFO_SIZE {
            return Err(BootInfoError::StructSize);
        }
        if self.flags & !BOOT_INFO_KNOWN_FLAGS != 0 {
            return Err(BootInfoError::Flags);
        }
        if self.reserved0 != [0; 6] || self.reserved1 != [0; 12] {
            return Err(BootInfoError::Reserved);
        }
        if self.kernel_entry == 0 || self.kernel_entry & 0x3 != 0 {
            return Err(BootInfoError::Alignment);
        }
        validate_pair(
            self.init_bundle_phys,
            self.init_bundle_len,
            ENEROS_INIT_BUNDLE_MAX_LEN,
        )?;
        // WP-37.6：bundle 必须落在 BOOT-007 内核映射窗口内（起始地址校验，
        // 旧实现只限长度——窗口外地址 root-task 无法读取）。
        if self.init_bundle_len != 0
            && (self.init_bundle_phys < ENEROS_INIT_BUNDLE_PHYS
                || self
                    .init_bundle_phys
                    .checked_add(self.init_bundle_len)
                    .map(|end| end > ENEROS_INIT_BUNDLE_PHYS + ENEROS_MAPPED_WINDOW_LEN)
                    .unwrap_or(true))
        {
            return Err(BootInfoError::Range);
        }
        validate_pair(self.dtb_phys, self.dtb_len, MAX_DTB_LEN)?;
        // WP-37.6：bundle 与 DTB 区间不得重叠（dtb_window_for 构造侧保证
        // DTB 在 bundle 之后页对齐；校验侧独立交叉检查，防构造漂移）。
        if self.init_bundle_len != 0 && self.dtb_len != 0 {
            let bundle_end = self
                .init_bundle_phys
                .checked_add(self.init_bundle_len)
                .ok_or(BootInfoError::Range)?;
            let dtb_end = self
                .dtb_phys
                .checked_add(self.dtb_len)
                .ok_or(BootInfoError::Range)?;
            let overlap = self.init_bundle_phys < dtb_end && self.dtb_phys < bundle_end;
            if overlap {
                return Err(BootInfoError::Overlap);
            }
        }
        validate_pair(
            self.memory_map_phys,
            self.memory_map_len,
            MAX_MEMORY_MAP_LEN,
        )?;
        if self.memory_map_entry_size != MemoryMapEntry::ENCODED_SIZE as u32 {
            return Err(BootInfoError::MemoryMap);
        }
        let required_map_len = u64::from(self.memory_map_count)
            .checked_mul(u64::from(self.memory_map_entry_size))
            .ok_or(BootInfoError::MemoryMap)?;
        if required_map_len > self.memory_map_len {
            return Err(BootInfoError::MemoryMap);
        }
        validate_pair(
            self.measurement_log_phys,
            self.measurement_log_len,
            MAX_MEASUREMENT_LOG_LEN,
        )?;
        if self.flags & BOOT_FLAG_MEASUREMENTS_VALID != 0 && self.measurement_log_len == 0 {
            return Err(BootInfoError::MeasurementLog);
        }
        if self.flags & BOOT_FLAG_ENTROPY_VALID != 0
            && self.entropy_seed.iter().all(|byte| *byte == 0)
        {
            return Err(BootInfoError::MissingEntropy);
        }
        if self.flags & BOOT_FLAG_SECURE_BOOT != 0
            && self.manifest_digest.iter().all(|byte| *byte == 0)
        {
            return Err(BootInfoError::MissingManifestDigest);
        }
        let mut copy = *self;
        let expected_crc = copy.crc32;
        copy.crc32 = 0;
        if expected_crc == 0 || crc32(&copy.to_le_bytes()) != expected_crc {
            return Err(BootInfoError::Crc);
        }
        Ok(())
    }

    pub fn to_le_bytes(&self) -> [u8; BOOT_INFO_SIZE] {
        let mut bytes = [0u8; BOOT_INFO_SIZE];
        bytes[0..8].copy_from_slice(&self.magic);
        put_u16(&mut bytes, 8, self.abi_major);
        put_u16(&mut bytes, 10, self.abi_minor);
        put_u32(&mut bytes, 12, self.struct_size);
        put_u64(&mut bytes, 16, self.flags);
        bytes[24..40].copy_from_slice(&self.boot_id);
        bytes[40] = self.selected_slot as u8;
        bytes[41] = self.boot_reason as u8;
        bytes[42..48].copy_from_slice(&self.reserved0);
        put_u64(&mut bytes, 48, self.kernel_entry);
        put_u64(&mut bytes, 56, self.init_bundle_phys);
        put_u64(&mut bytes, 64, self.init_bundle_len);
        put_u64(&mut bytes, 72, self.dtb_phys);
        put_u64(&mut bytes, 80, self.dtb_len);
        put_u64(&mut bytes, 88, self.memory_map_phys);
        put_u64(&mut bytes, 96, self.memory_map_len);
        put_u32(&mut bytes, 104, self.memory_map_count);
        put_u32(&mut bytes, 108, self.memory_map_entry_size);
        put_u64(&mut bytes, 112, self.measurement_log_phys);
        put_u64(&mut bytes, 120, self.measurement_log_len);
        bytes[128..192].copy_from_slice(&self.entropy_seed);
        bytes[192..224].copy_from_slice(&self.manifest_digest);
        put_u64(&mut bytes, 224, self.bootctrl_generation);
        put_u64(&mut bytes, 232, self.security_version);
        put_u32(&mut bytes, CRC_OFFSET, self.crc32);
        bytes[244..256].copy_from_slice(&self.reserved1);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BootInfoError> {
        if bytes.len() != BOOT_INFO_SIZE {
            return Err(BootInfoError::Length);
        }
        let mut fixed = [0u8; BOOT_INFO_SIZE];
        fixed.copy_from_slice(bytes);
        let mut info = Self::new(
            BootSlot::try_from(fixed[40])?,
            BootReason::try_from(fixed[41])?,
        );
        info.magic.copy_from_slice(&fixed[0..8]);
        info.abi_major = get_u16(&fixed, 8);
        info.abi_minor = get_u16(&fixed, 10);
        info.struct_size = get_u32(&fixed, 12);
        info.flags = get_u64(&fixed, 16);
        info.boot_id.copy_from_slice(&fixed[24..40]);
        info.reserved0.copy_from_slice(&fixed[42..48]);
        info.kernel_entry = get_u64(&fixed, 48);
        info.init_bundle_phys = get_u64(&fixed, 56);
        info.init_bundle_len = get_u64(&fixed, 64);
        info.dtb_phys = get_u64(&fixed, 72);
        info.dtb_len = get_u64(&fixed, 80);
        info.memory_map_phys = get_u64(&fixed, 88);
        info.memory_map_len = get_u64(&fixed, 96);
        info.memory_map_count = get_u32(&fixed, 104);
        info.memory_map_entry_size = get_u32(&fixed, 108);
        info.measurement_log_phys = get_u64(&fixed, 112);
        info.measurement_log_len = get_u64(&fixed, 120);
        info.entropy_seed.copy_from_slice(&fixed[128..192]);
        info.manifest_digest.copy_from_slice(&fixed[192..224]);
        info.bootctrl_generation = get_u64(&fixed, 224);
        info.security_version = get_u64(&fixed, 232);
        info.crc32 = get_u32(&fixed, CRC_OFFSET);
        info.reserved1.copy_from_slice(&fixed[244..256]);
        info.validate()?;
        Ok(info)
    }
}

fn validate_pair(address: u64, length: u64, maximum: u64) -> Result<(), BootInfoError> {
    if length == 0 {
        if address != 0 {
            return Err(BootInfoError::Range);
        }
        return Ok(());
    }
    if address == 0 || length > maximum || address.checked_add(length).is_none() {
        return Err(BootInfoError::Range);
    }
    Ok(())
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn get_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn get_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

fn crc32(bytes: &[u8]) -> u32 {
    // HYG-001：CRC32 唯一实现归一到 abi/ipc-wire（IEEE 802.3 0xEDB88320）。
    eneros_ipc_wire::crc32(bytes)
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::mem::{align_of, offset_of, size_of};

    use super::*;

    fn valid_info() -> BootInfo {
        let mut info = BootInfo::new(BootSlot::A, BootReason::Cold);
        info.flags = BOOT_FLAG_SECURE_BOOT | BOOT_FLAG_ENTROPY_VALID | BOOT_FLAG_MEASUREMENTS_VALID;
        info.boot_id = [0x11; 16];
        info.kernel_entry = 0x4100_0000;
        info.init_bundle_phys = ENEROS_INIT_BUNDLE_PHYS;
        info.init_bundle_len = 0x2000;
        info.dtb_phys = dtb_window_for(0x2000).unwrap().0;
        info.dtb_len = 0x1000;
        info.memory_map_phys = 0x4400_0000;
        info.memory_map_len = 64;
        info.memory_map_count = 2;
        info.measurement_log_phys = 0x4500_0000;
        info.measurement_log_len = 0x400;
        info.entropy_seed = [0x5A; 64];
        info.manifest_digest = [0xA5; 32];
        info.bootctrl_generation = 7;
        info.security_version = 3;
        info.seal();
        info
    }

    #[test]
    fn layout_is_exact_and_cross_pointer_width_independent() {
        assert_eq!(size_of::<BootInfo>(), 256);
        assert_eq!(align_of::<BootInfo>(), 16);
        assert_eq!(offset_of!(BootInfo, flags), 16);
        assert_eq!(offset_of!(BootInfo, selected_slot), 40);
        assert_eq!(offset_of!(BootInfo, kernel_entry), 48);
        assert_eq!(offset_of!(BootInfo, entropy_seed), 128);
        assert_eq!(offset_of!(BootInfo, manifest_digest), 192);
        assert_eq!(offset_of!(BootInfo, crc32), 240);
        assert_eq!(size_of::<MemoryMapEntry>(), 32);
    }

    #[test]
    fn golden_little_endian_roundtrip() {
        let info = valid_info();
        let bytes = info.to_le_bytes();
        assert_eq!(&bytes[0..8], b"ENRBOOT\0");
        assert_eq!(&bytes[8..16], &[1, 0, 0, 0, 0, 1, 0, 0]);
        assert_eq!(&bytes[48..56], &0x4100_0000u64.to_le_bytes());
        assert_eq!(&bytes[224..232], &7u64.to_le_bytes());
        assert_eq!(BootInfo::decode(&bytes), Ok(info));
    }

    #[test]
    fn every_single_byte_corruption_is_detected() {
        let bytes = valid_info().to_le_bytes();
        for index in 0..bytes.len() {
            let mut corrupted = bytes;
            corrupted[index] ^= 0x01;
            assert!(
                BootInfo::decode(&corrupted).is_err(),
                "single-byte corruption at {index} was accepted"
            );
        }
    }

    #[test]
    fn unknown_version_flags_and_overflow_fail_closed() {
        let mut info = valid_info();
        info.abi_major = 2;
        info.seal();
        assert_eq!(info.validate(), Err(BootInfoError::Version));

        let mut info = valid_info();
        info.flags |= 1 << 63;
        info.seal();
        assert_eq!(info.validate(), Err(BootInfoError::Flags));

        let mut info = valid_info();
        info.init_bundle_phys = u64::MAX - 7;
        info.init_bundle_len = 16;
        info.seal();
        assert_eq!(info.validate(), Err(BootInfoError::Range));
    }

    #[test]
    fn memory_map_and_entropy_semantics_are_checked() {
        let mut info = valid_info();
        info.memory_map_count = 3;
        info.seal();
        assert_eq!(info.validate(), Err(BootInfoError::MemoryMap));

        let mut info = valid_info();
        info.entropy_seed = [0; 64];
        info.seal();
        assert_eq!(info.validate(), Err(BootInfoError::MissingEntropy));
    }

    fn encoded_map(entries: &[MemoryMapEntry]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for entry in entries {
            bytes.extend_from_slice(&entry.to_le_bytes().unwrap());
        }
        bytes
    }

    #[test]
    fn multiple_banks_and_reserved_regions_are_parsed() {
        let entries = [
            MemoryMapEntry {
                base: 0x4000_0000,
                length: 0x1000_0000,
                kind: MemoryRegionKind::Usable,
                attributes: 0,
                reserved: 0,
            },
            MemoryMapEntry {
                base: 0x0900_0000,
                length: 0x1000,
                kind: MemoryRegionKind::Mmio,
                attributes: 0,
                reserved: 0,
            },
            MemoryMapEntry {
                base: 0x5000_0000,
                length: 0x0800_0000,
                kind: MemoryRegionKind::Reserved,
                attributes: 0,
                reserved: 0,
            },
            MemoryMapEntry {
                base: 0x8000_0000,
                length: 0x2000_0000,
                kind: MemoryRegionKind::Usable,
                attributes: 0,
                reserved: 0,
            },
        ];
        let bytes = encoded_map(&entries);
        assert_eq!(validate_memory_map_bytes(&bytes, 4), Ok(()));
        assert_eq!(MemoryMapEntry::decode(&bytes[..32]), Ok(entries[0]));
    }

    #[test]
    fn memory_map_overlap_overflow_unknown_kind_and_reserved_bits_fail_closed() {
        let first = MemoryMapEntry {
            base: 0x4000_0000,
            length: 0x2000,
            kind: MemoryRegionKind::Usable,
            attributes: 0,
            reserved: 0,
        };
        let overlapping = MemoryMapEntry {
            base: 0x4000_1000,
            length: 0x2000,
            kind: MemoryRegionKind::Reserved,
            attributes: 0,
            reserved: 0,
        };
        let bytes = encoded_map(&[first, overlapping]);
        assert_eq!(
            validate_memory_map_bytes(&bytes, 2),
            Err(BootInfoError::MemoryMap)
        );
        assert_eq!(
            validate_memory_map_bytes(&bytes[..32], 2),
            Err(BootInfoError::MemoryMap)
        );

        let mut unknown_kind = first.to_le_bytes().unwrap();
        put_u32(&mut unknown_kind, 16, 99);
        assert_eq!(
            MemoryMapEntry::decode(&unknown_kind),
            Err(BootInfoError::MemoryMap)
        );

        let mut reserved = first.to_le_bytes().unwrap();
        put_u64(&mut reserved, 24, 1);
        assert_eq!(
            MemoryMapEntry::decode(&reserved),
            Err(BootInfoError::Reserved)
        );

        let overflow = MemoryMapEntry {
            base: u64::MAX - 0xff,
            length: 0x100,
            kind: MemoryRegionKind::Usable,
            attributes: 0,
            reserved: 0,
        };
        assert_eq!(overflow.to_le_bytes(), Err(BootInfoError::Range));
    }

    /// WP-37.6：bundle 必须在 BOOT-007 窗口内（旧实现只限长度，窗口外
    /// 地址 root-task 无法读取却通过校验）。
    #[test]
    fn validate_rejects_bundle_outside_boot007_window() {
        // 起始地址在窗口之前。
        let mut info = valid_info();
        info.init_bundle_phys = ENEROS_INIT_BUNDLE_PHYS - 0x1000;
        info.seal();
        assert_eq!(info.validate(), Err(BootInfoError::Range));
        // 末端超出窗口（起始合法但长度越窗）。
        let mut info2 = valid_info();
        info2.init_bundle_len = ENEROS_MAPPED_WINDOW_LEN + 1;
        info2.seal();
        assert_eq!(info2.validate(), Err(BootInfoError::Range));
        // 窗口内合法位置照常通过（正例）。
        let ok = valid_info();
        assert_eq!(ok.validate(), Ok(()));
    }

    /// WP-37.6：bundle 与 DTB 区间重叠必须拒绝（构造侧 dtb_window_for
    /// 保证不重叠；校验侧独立交叉检查防构造漂移）。
    #[test]
    fn validate_rejects_bundle_dtb_overlap() {
        let mut info = valid_info();
        // DTB 起点压在 bundle 区间内部。
        info.dtb_phys = info.init_bundle_phys + 0x1000;
        info.seal();
        assert_eq!(info.validate(), Err(BootInfoError::Overlap));
        // DTB 完全包含 bundle（反向重叠）。
        let mut info2 = valid_info();
        info2.dtb_phys = ENEROS_INIT_BUNDLE_PHYS - 0x1000;
        info2.dtb_len = 0x4000;
        info2.seal();
        assert_eq!(info2.validate(), Err(BootInfoError::Overlap));
    }
}
