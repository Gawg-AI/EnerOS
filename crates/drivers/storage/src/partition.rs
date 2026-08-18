//! 块设备角色化发现/识别/授权写分离（蓝图 39 WP-39.1~39.3）。
//!
//! # 状态机（零写原则）
//!
//! ```text
//! Discovered(read-only)
//!   → Identified(read-only metadata + GPT)
//!   → RoleMatched(policy + type GUID)
//!   → Claimed(PartitionHandle<R>——exclusive owner)
//!   → MountedExisting OR ProvisioningAuthorized
//!   → Formatted(explicit only, 经 ProvisioningGate)
//!   → Ready
//! ```
//!
//! Discovery/Identify/RoleMatch 阶段**绝不调用** format、repair、journal
//! replay with writes 或自动初始化 header。写盘只允许两种路径：
//! ① 已 Claimed 的 `PartitionHandle`（正常业务读写，含已存在卷的挂载）；
//! ② 显式 `ProvisioningGate::authorize` 产出的 `ProvisionAuthorization`
//!    （factory/recovery profile 专属；normal release 无此 capability）。
//!
//! # 类型护栏
//!
//! - `PartitionHandle<R: Role, D>` 携带 role typestate：`PartitionHandle<Data>`
//!   只能由 `GptResolver::open::<Data>` 构造——拿不到 role 匹配 = 编译期
//!   拿不到句柄（蓝图 35-00 §10.2：类型化权限）。
//! - LBA 翻译全部 checked；句柄永不越出分区 slice。
//! - 发现快照可经 [`BlockTrace`] 记录所有 read/write LBA；测试断言
//!   discovery 场景 write count == 0。

use alloc::vec;
use alloc::vec::Vec;

use eneros_image_format::gpt::Guid;
use eneros_image_format::media::{BlockDevice as GptMedia, BlockReadError};

use crate::driver::BlockDevice;
use crate::error::StorageError;

// ============================================================================
// BlockRole 与 role typestate
// ============================================================================

/// 分区业务角色（蓝图 39 WP-39.1 类型模型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockRole {
    Boot,
    SystemA,
    SystemB,
    Recovery,
    Data,
    Audit,
    ModelA,
    ModelB,
    Evidence,
}

impl BlockRole {
    /// 角色短名（机器可读、稳定——用于证据与 policy 键）。
    pub const fn name(&self) -> &'static str {
        match self {
            BlockRole::Boot => "boot",
            BlockRole::SystemA => "system-a",
            BlockRole::SystemB => "system-b",
            BlockRole::Recovery => "recovery",
            BlockRole::Data => "data",
            BlockRole::Audit => "audit",
            BlockRole::ModelA => "model-a",
            BlockRole::ModelB => "model-b",
            BlockRole::Evidence => "evidence",
        }
    }
}

/// 全部角色（policy 校验与证据枚举用）。
pub const ALL_ROLES: [BlockRole; 9] = [
    BlockRole::Boot,
    BlockRole::SystemA,
    BlockRole::SystemB,
    BlockRole::Recovery,
    BlockRole::Data,
    BlockRole::Audit,
    BlockRole::ModelA,
    BlockRole::ModelB,
    BlockRole::Evidence,
];

mod sealed {
    /// 角色 typestate 密封（禁止外部实现新 Role——角色集合封闭）。
    pub trait Sealed {}
}

/// 分区角色 typestate（`PartitionHandle<R>` 的类型参数）。
pub trait Role: sealed::Sealed {
    /// 该 typestate 对应的 [`BlockRole`]。
    const ROLE: BlockRole;
}

macro_rules! role_types {
    ($($ty:ident => $role:expr),+ $(,)?) => {
        $(
            /// Role typestate（见 [`Role`]）。
            pub struct $ty;
            impl sealed::Sealed for $ty {}
            impl Role for $ty {
                const ROLE: BlockRole = $role;
            }
        )+
    };
}

role_types! {
    BootRole => BlockRole::Boot,
    SystemARole => BlockRole::SystemA,
    SystemBRole => BlockRole::SystemB,
    RecoveryRole => BlockRole::Recovery,
    DataRole => BlockRole::Data,
    AuditRole => BlockRole::Audit,
    ModelARole => BlockRole::ModelA,
    ModelBRole => BlockRole::ModelB,
    EvidenceRole => BlockRole::Evidence,
}

// ============================================================================
// LbaRange / BlockIdentity / PartitionIdentity
// ============================================================================

/// 分区内的 LBA 范围（checked 算术）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LbaRange {
    /// 分区起始 LBA（绝对）。
    pub start_lba: u64,
    /// 分区块数。
    pub block_count: u64,
}

impl LbaRange {
    /// 构造（`start + count` 溢出 → None）。
    pub const fn new(start_lba: u64, block_count: u64) -> Option<Self> {
        if block_count == 0 {
            return None;
        }
        match start_lba.checked_add(block_count) {
            Some(_) => Some(Self {
                start_lba,
                block_count,
            }),
            None => None,
        }
    }

    /// 分区内第 `index` 块的绝对 LBA（越界 → None）。
    pub const fn translate(&self, index: u64) -> Option<u64> {
        if index >= self.block_count {
            return None;
        }
        match self.start_lba.checked_add(index) {
            Some(lba) => Some(lba),
            None => None,
        }
    }

    /// 绝对 LBA 是否落在分区内。
    pub const fn contains(&self, lba: u64) -> bool {
        lba >= self.start_lba
            && match self.start_lba.checked_add(self.block_count) {
                Some(end) => lba < end,
                None => false,
            }
    }

    /// 结束 LBA（exclusive；溢出 → None）。
    pub const fn end_exclusive(&self) -> Option<u64> {
        self.start_lba.checked_add(self.block_count)
    }

    /// 与另一范围是否重叠（含相邻——相邻视为不重叠）。
    pub const fn overlaps(&self, other: &LbaRange) -> bool {
        match (self.end_exclusive(), other.end_exclusive()) {
            (Some(self_end), Some(other_end)) => {
                self.start_lba < other_end && other.start_lba < self_end
            }
            _ => true, // 溢出视为重叠（保守拒绝）
        }
    }
}

/// 块设备稳定身份（发现结果；零写）。
#[derive(Debug, Clone)]
pub struct BlockIdentity {
    /// 平台 inventory 设备实例 id。
    pub device_id: u32,
    /// 稳定设备路径（固定缓冲；`stable_path_len` 为有效字节数）。
    pub stable_path: [u8; 64],
    /// `stable_path` 有效字节数。
    pub stable_path_len: u8,
    /// 设备容量（字节）。
    pub capacity_bytes: u64,
    /// 逻辑扇区大小（字节）。
    pub logical_sector: u32,
    /// 物理扇区大小（字节）。
    pub physical_sector: u32,
    /// GPT 磁盘 GUID（非 GPT 盘为 None）。
    pub gpt_disk_guid: Option<Guid>,
    /// 介质序列号哈希（不可用时 None）。
    pub media_serial_hash: Option<[u8; 32]>,
}

impl BlockIdentity {
    /// 从字节构造稳定路径（超长截断，fail-safe）。
    pub fn with_stable_path(mut self, path: &[u8]) -> Self {
        let len = path.len().min(self.stable_path.len());
        self.stable_path[..len].copy_from_slice(&path[..len]);
        self.stable_path_len = len as u8;
        self
    }

    /// 稳定路径（`&str` 视图；UTF-8 不合法时为空串）。
    pub fn stable_path_str(&self) -> &str {
        core::str::from_utf8(&self.stable_path[..self.stable_path_len as usize]).unwrap_or("")
    }
}

/// 已识别分区身份（role 匹配后）。
#[derive(Debug, Clone, PartialEq)]
pub struct PartitionIdentity {
    /// 分区名（GPT name，固定缓冲）。
    pub name: [u8; 64],
    /// `name` 有效字节数。
    pub name_len: u8,
    /// 分区 type GUID。
    pub type_guid: Guid,
    /// 分区 unique GUID。
    pub unique_guid: Guid,
    /// 业务角色（policy 裁决）。
    pub role: BlockRole,
    /// 分区 LBA 范围（**512B 扇区空间**——GPT 定义）。
    pub range: LbaRange,
    /// 每存储块含 512B 扇区数（1 = 512B 设备；8 = 4Kn 设备）。
    pub sectors_per_block: u32,
}

impl PartitionIdentity {
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }
}

// ============================================================================
// RolePolicy
// ============================================================================

/// type GUID → 业务角色 的映射策略（来自签名 ProductManifest 的分区政策；
/// 39 提供默认映射并对齐 `product/image-layouts/qemu-v1.json`）。
#[derive(Debug, Clone)]
pub struct RolePolicy {
    entries: Vec<(Guid, BlockRole)>,
}

impl RolePolicy {
    /// 构造（重复 type GUID → 取后一个；调用方负责按 manifest 校验）。
    pub fn new(entries: Vec<(Guid, BlockRole)>) -> Self {
        Self { entries }
    }

    /// 查询 type GUID 对应的角色（未登记 → None：外来/无关分区跳过，
    /// 不阻断识别，也绝不猜测角色）。
    pub fn lookup(&self, type_guid: &Guid) -> Option<BlockRole> {
        self.entries
            .iter()
            .find(|(guid, _)| guid == type_guid)
            .map(|(_, role)| *role)
    }

    /// 角色是否已登记。
    pub fn contains_role(&self, role: BlockRole) -> bool {
        self.entries.iter().any(|(_, r)| *r == role)
    }
}

impl Default for RolePolicy {
    /// 默认政策：对齐 qemu-v1 布局的 EnerOS 命名空间
    /// `454e4552-000N-4001-8000-00000000000N`。
    ///
    /// 映射：PLATFORM_BOOT(ESP)→Boot、BOOT_A→SystemA、BOOT_B→SystemB、
    /// RECOVERY→Recovery、DATA→Data、LOGS→Audit。
    /// BOOTCTRL/SYS_A/SYS_B/CONFIG 不在角色模型内 → 不映射（跳过）。
    fn default() -> Self {
        // canonical 字节 = 文本逐字节：454e4552-00 0N -40 01 -8000-0000000000 0N
        let guid = |n: u8| {
            let mut g = [0u8; 16];
            g[0] = 0x45;
            g[1] = 0x4e;
            g[2] = 0x45;
            g[3] = 0x52;
            g[4] = 0x00;
            g[5] = n;
            g[6] = 0x40;
            g[7] = 0x01;
            g[8] = 0x80;
            g[9] = 0x00;
            g[15] = n;
            Guid(g)
        };
        let esp =
            Guid::parse("c12a7328-f81f-11d2-ba4b-00a0c93ec93b").expect("static ESP GUID parses");
        Self::new(vec![
            (esp, BlockRole::Boot),
            (guid(0x03), BlockRole::SystemA),
            (guid(0x04), BlockRole::SystemB),
            (guid(0x07), BlockRole::Recovery),
            (guid(0x09), BlockRole::Data),
            (guid(0x0a), BlockRole::Audit),
        ])
    }
}

// ============================================================================
// GPT 适配与 resolver
// ============================================================================

/// GPT 解析错误（fail-closed 语义：任何异常都不产生句柄）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionResolveError {
    /// 底层块 I/O 失败。
    Io,
    /// 设备容量为 0（不可识别）。
    CapacityZero,
    /// 块大小不是 512 的倍数或超过 4096（GPT 逻辑扇区要求）。
    UnsupportedGeometry,
    /// 无 GPT / GPT 校验失败（MBR、坏 CRC、越界——image-format 判定）。
    NoGpt,
    /// 分区表存在重复 unique GUID（拒绝，防混淆）。
    DuplicateGuid,
    /// 分区越出设备范围。
    OutOfBounds,
    /// 分区范围相互重叠。
    Overlap,
    /// 目标角色不存在（找不到该角色的分区）。
    RoleNotFound,
}

/// 把 storage [`BlockDevice`] 适配为 image-format GPT 读取面。
///
/// GPT 逻辑扇区恒为 512B（含 4Kn 设备，规范规定 GPT 使用 512B LBA
/// 寻址）；适配层按字节范围换算到 storage 块并整块读入后切片。
struct GptDevice<'a, D: BlockDevice> {
    inner: &'a mut D,
}

impl<D: BlockDevice> GptDevice<'_, D> {
    fn block_size(&self) -> Result<usize, BlockReadError> {
        let bs = self.inner.block_size();
        if bs == 0 || bs > 4096 || bs % 512 != 0 {
            return Err(BlockReadError::SectorSize);
        }
        Ok(bs)
    }
}

impl<D: BlockDevice> GptMedia for GptDevice<'_, D> {
    fn sector_size(&self) -> u32 {
        512
    }

    fn total_lbas(&self) -> u64 {
        // 容量 0 的防御在 resolver 处理；这里只做换算。
        let bytes = self
            .inner
            .block_count()
            .saturating_mul(self.inner.block_size() as u64);
        bytes / 512
    }

    fn read_blocks(&mut self, first_lba: u64, buffer: &mut [u8]) -> Result<(), BlockReadError> {
        let bs = self.block_size()?;
        if buffer.is_empty() {
            return Ok(());
        }
        let byte_start = first_lba.checked_mul(512).ok_or(BlockReadError::Overflow)?;
        let byte_end = byte_start
            .checked_add(buffer.len() as u64)
            .ok_or(BlockReadError::Overflow)?;
        let total_bytes = self
            .inner
            .block_count()
            .checked_mul(bs as u64)
            .ok_or(BlockReadError::Overflow)?;
        if byte_end > total_bytes {
            return Err(BlockReadError::OutOfBounds);
        }
        // GPT 逻辑扇区 512B 可不对齐存储块（4Kn 设备）——整块读入 scratch
        // 后按字节区间切片，buffer 任意长度（≤4096）都支持。
        let first_block = byte_start / bs as u64;
        let last_block = (byte_end - 1) / bs as u64;
        let mut scratch = [0u8; 4096];
        let mut out_pos = 0usize;
        for block in first_block..=last_block {
            self.inner
                .read_block(block, &mut scratch[..bs])
                .map_err(|_| BlockReadError::Media)?;
            let block_start = block * bs as u64;
            let block_end = block_start + bs as u64;
            // 请求字节区间 [byte_start, byte_end) 与该块的交集。
            let lo = byte_start.max(block_start) - block_start;
            let hi = byte_end.min(block_end) - block_start;
            let len = (hi - lo) as usize;
            buffer[out_pos..out_pos + len].copy_from_slice(&scratch[lo as usize..hi as usize]);
            out_pos += len;
        }
        Ok(())
    }
}

/// GPT 分区解析器（零写：只经 `read_block`；发现阶段从不写盘）。
pub struct GptResolver;

impl GptResolver {
    /// 读取并校验 GPT（双份头 CRC/bounds/重复 GUID/重叠），按 policy
    /// 映射角色，返回全部已识别分区身份。
    pub fn resolve<D: BlockDevice>(
        dev: &mut D,
        policy: &RolePolicy,
    ) -> Result<Vec<PartitionIdentity>, PartitionResolveError> {
        if dev.block_count() == 0 {
            return Err(PartitionResolveError::CapacityZero);
        }
        let bs = dev.block_size();
        if bs == 0 || bs > 4096 || bs % 512 != 0 {
            return Err(PartitionResolveError::UnsupportedGeometry);
        }
        let mut adapter = GptDevice { inner: dev };
        // 39 收尾复验：具体错误透传（重复 GUID/越界/重叠不得被吞为
        // 泛化 NoGpt——证据与诊断必须可区分）。
        let gpt = eneros_image_format::gpt::read_gpt(&mut adapter).map_err(|e| {
            use eneros_image_format::gpt::GptError;
            match e {
                GptError::DuplicateGuid => PartitionResolveError::DuplicateGuid,
                GptError::PartitionOverlap => PartitionResolveError::Overlap,
                GptError::PartitionBounds => PartitionResolveError::OutOfBounds,
                _ => PartitionResolveError::NoGpt,
            }
        })?;
        let sectors_per_block = (bs as u32) / 512;

        let mut out: Vec<PartitionIdentity> = Vec::new();
        let mut seen_guids: Vec<Guid> = Vec::new();
        let mut seen_ranges: Vec<LbaRange> = Vec::new();
        for index in 0..usize::from(gpt.partition_count) {
            let part = gpt.partition(index).ok_or(PartitionResolveError::NoGpt)?;
            if part.type_guid == Guid::ZERO && part.unique_guid == Guid::ZERO {
                continue; // 空条目
            }
            let Some(role) = policy.lookup(&part.type_guid) else {
                continue; // 未登记角色 → 跳过（不猜）
            };
            // bounds：first..last 必须在设备范围内（GPT 扇区空间）。
            if part.first_lba >= gpt.total_lbas || part.last_lba >= gpt.total_lbas {
                return Err(PartitionResolveError::OutOfBounds);
            }
            let block_count = part
                .last_lba
                .checked_sub(part.first_lba)
                .and_then(|v| v.checked_add(1))
                .ok_or(PartitionResolveError::OutOfBounds)?;
            let range = LbaRange::new(part.first_lba, block_count)
                .ok_or(PartitionResolveError::OutOfBounds)?;
            // 重复 unique GUID（防两个分区身份混淆）。
            if seen_guids.contains(&part.unique_guid) {
                return Err(PartitionResolveError::DuplicateGuid);
            }
            // 重叠拒绝（分区不得重叠）。
            if seen_ranges.iter().any(|r| r.overlaps(&range)) {
                return Err(PartitionResolveError::Overlap);
            }
            seen_guids.push(part.unique_guid);
            seen_ranges.push(range);

            let mut name = [0u8; 64];
            let mut name_len = 0usize;
            for (i, unit) in part.name[..usize::from(part.name_len)].iter().enumerate() {
                if i >= name.len() {
                    break;
                }
                name[i] = (*unit & 0xFF) as u8; // UTF-16LE 低字节（ASCII 分区名）
                name_len += 1;
            }
            out.push(PartitionIdentity {
                name,
                name_len: name_len as u8,
                type_guid: part.type_guid,
                unique_guid: part.unique_guid,
                role,
                range,
                sectors_per_block,
            });
        }
        Ok(out)
    }

    /// 解析并打开指定角色分区（role typestate：只有目标角色能拿到句柄）。
    pub fn open<R: Role, D: BlockDevice>(
        mut dev: D,
        policy: &RolePolicy,
    ) -> Result<PartitionHandle<R, D>, PartitionResolveError> {
        let identities = Self::resolve(&mut dev, policy)?;
        let identity = identities
            .into_iter()
            .find(|p| p.role == R::ROLE)
            .ok_or(PartitionResolveError::RoleNotFound)?;
        let spb = u64::from(identity.sectors_per_block);
        // 4Kn：分区起始与长度必须是存储块对齐（否则无法整块寻址——
        // 不允许部分块访问，fail-closed）。
        if identity.range.start_lba % spb != 0 || identity.range.block_count % spb != 0 {
            return Err(PartitionResolveError::UnsupportedGeometry);
        }
        let block_size = dev.block_size();
        Ok(PartitionHandle {
            dev,
            identity,
            block_size,
            _role: core::marker::PhantomData,
        })
    }
}

// ============================================================================
// PartitionHandle
// ============================================================================

/// 已认领分区的排他句柄（role typestate；checked LBA 翻译）。
///
/// 写语义：句柄构造即代表"已 Claimed"——此后读写是正常业务操作
/// （挂载已存在卷/追加日志）；"发现即写盘"被类型上排除（发现阶段
/// 没有任何写 API，只有 [`GptResolver::resolve`] 的只读路径）。
pub struct PartitionHandle<R: Role, D: BlockDevice> {
    dev: D,
    /// 分区身份（含 role/range）。
    identity: PartitionIdentity,
    block_size: usize,
    _role: core::marker::PhantomData<R>,
}

impl<R: Role, D: BlockDevice> PartitionHandle<R, D> {
    /// 分区身份。
    pub fn identity(&self) -> &PartitionIdentity {
        &self.identity
    }

    /// 分区 LBA 范围。
    pub fn range(&self) -> LbaRange {
        self.identity.range
    }

    /// 分区内块数（存储块空间 = 扇区数 / spb）。
    pub fn block_count(&self) -> u64 {
        self.identity.range.block_count / u64::from(self.identity.sectors_per_block)
    }

    /// 块大小（字节）。
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// 分区内第 `index` 块 → 绝对存储块 LBA（checked；越界 None）。
    /// 分区起始已对齐（open 时校验），绝对扇区 = start + index × spb。
    fn translate(&self, index: u64) -> Result<u64, StorageError> {
        if index >= self.block_count() {
            return Err(StorageError::OutOfRange {
                block_idx: index,
                max: self.block_count(),
            });
        }
        let spb = u64::from(self.identity.sectors_per_block);
        let sector = self.identity.range.start_lba + index * spb;
        Ok(sector / spb)
    }

    /// 读分区内第 `index` 块（永不越出 slice）。
    pub fn read_block(&self, index: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        if buf.len() != self.block_size {
            return Err(StorageError::Unsupported {
                detail: "partition read: buffer size != block size",
            });
        }
        self.dev.read_block(self.translate(index)?, buf)
    }

    /// 写分区内第 `index` 块（Claimed 后允许；LBA 翻译 checked）。
    pub fn write_block(&mut self, index: u64, buf: &[u8]) -> Result<(), StorageError> {
        if buf.len() != self.block_size {
            return Err(StorageError::Unsupported {
                detail: "partition write: buffer size != block size",
            });
        }
        self.dev.write_block(self.translate(index)?, buf)
    }

    /// 擦除分区内第 `index` 块。
    pub fn erase_block(&mut self, index: u64) -> Result<(), StorageError> {
        self.dev.erase_block(self.translate(index)?)
    }

    /// flush（写入持久化到介质）。
    pub fn flush(&mut self) -> Result<(), StorageError> {
        self.dev.flush()
    }

    /// 底层设备健康状态。
    pub fn health_status(&self) -> crate::driver::DeviceHealth {
        self.dev.health_status()
    }

    /// 释放句柄，取回底层设备。
    pub fn into_inner(self) -> D {
        self.dev
    }
}

/// `PartitionHandle` 本身即 [`BlockDevice`]——DurableLog/FS/TSDB 的
/// `D: BlockDevice` 泛型可以直接接受 role 句柄（role 在句柄构造时
/// 由 typestate 强制，业务层拿不到未认领的设备）。
impl<R: Role, D: BlockDevice> BlockDevice for PartitionHandle<R, D> {
    fn read_block(&self, block_idx: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        self.read_block(block_idx, buf)
    }

    fn write_block(&mut self, block_idx: u64, buf: &[u8]) -> Result<(), StorageError> {
        self.write_block(block_idx, buf)
    }

    fn erase_block(&mut self, block_idx: u64) -> Result<(), StorageError> {
        self.erase_block(block_idx)
    }

    fn block_count(&self) -> u64 {
        self.block_count()
    }

    fn block_size(&self) -> usize {
        self.block_size
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        self.flush()
    }

    fn health_status(&self) -> crate::driver::DeviceHealth {
        self.health_status()
    }
}

impl<R: Role, D: BlockDevice> core::fmt::Debug for PartitionHandle<R, D> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PartitionHandle")
            .field("role", &R::ROLE.name())
            .field("identity", &self.identity)
            .finish()
    }
}

// ============================================================================
// ScopedBlockDevice（Exit Gate #2：DurableLog/FS/TSDB 只接受 role 分区句柄）
// ============================================================================

mod scope_sealed {
    /// 作用域设备密封——仅 [`super::PartitionHandle`] 与测试用
    /// [`super::MockPartitionScope`] 可实现（整盘裸驱动拿不到作用域）。
    pub trait Sealed {}
}

/// 带业务角色作用域的块设备（蓝图 39 WP-39.1/Exit Gate #2）。
///
/// **密封 trait**：只有角色分区句柄（[`PartitionHandle`]）能实现；
/// DurableLog/FS/TSDB 的公开构造以此 trait 为界——整盘裸 handle
///（含 LBA0/GPT 区）在类型上无法进入业务存储层。
pub trait ScopedBlockDevice: BlockDevice + scope_sealed::Sealed {
    /// 本作用域的业务角色。
    fn scope_role(&self) -> BlockRole;
}

impl<R: Role, D: BlockDevice> scope_sealed::Sealed for PartitionHandle<R, D> {}

impl<R: Role, D: BlockDevice> ScopedBlockDevice for PartitionHandle<R, D> {
    fn scope_role(&self) -> BlockRole {
        R::ROLE
    }
}

/// 测试/host 仿真用作用域包装（`mock` feature——release 熔断，
/// 不可能进入正式产物）。
///
/// 语义：把任意块设备包装为"已认领的分区作用域"，仅供单测构造
/// DurableLog/FS/TSDB；target/release 装配必须经 [`GptResolver::open`]。
#[cfg(any(test, feature = "mock"))]
pub struct MockPartitionScope<D: BlockDevice> {
    inner: D,
    role: BlockRole,
}

#[cfg(any(test, feature = "mock"))]
impl<D: BlockDevice> MockPartitionScope<D> {
    /// 构造测试作用域（role 仅测试语义；不承担安全授权）。
    pub fn new(inner: D, role: BlockRole) -> Self {
        Self { inner, role }
    }

    /// 取回内部设备。
    pub fn into_inner(self) -> D {
        self.inner
    }
}

#[cfg(any(test, feature = "mock"))]
impl<D: BlockDevice> BlockDevice for MockPartitionScope<D> {
    fn read_block(&self, block_idx: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        self.inner.read_block(block_idx, buf)
    }

    fn write_block(&mut self, block_idx: u64, buf: &[u8]) -> Result<(), StorageError> {
        self.inner.write_block(block_idx, buf)
    }

    fn erase_block(&mut self, block_idx: u64) -> Result<(), StorageError> {
        self.inner.erase_block(block_idx)
    }

    fn block_count(&self) -> u64 {
        self.inner.block_count()
    }

    fn block_size(&self) -> usize {
        self.inner.block_size()
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        self.inner.flush()
    }

    fn health_status(&self) -> crate::driver::DeviceHealth {
        self.inner.health_status()
    }
}

#[cfg(any(test, feature = "mock"))]
impl<D: BlockDevice> scope_sealed::Sealed for MockPartitionScope<D> {}

#[cfg(any(test, feature = "mock"))]
impl<D: BlockDevice> ScopedBlockDevice for MockPartitionScope<D> {
    fn scope_role(&self) -> BlockRole {
        self.role
    }
}

/// 测试便捷：构造 boxed 作用域设备（`mock` feature——release 熔断）。
///
/// target 装配不得使用；生产路径必须经 [`GptResolver::open`]。
#[cfg(any(test, feature = "mock"))]
pub fn boxed_mock_scope<D: BlockDevice + 'static>(
    inner: D,
    role: BlockRole,
) -> alloc::boxed::Box<dyn ScopedBlockDevice> {
    alloc::boxed::Box::new(MockPartitionScope::new(inner, role))
}

// ============================================================================
// BlockTrace（零写发现快照）
// ============================================================================

/// 记录设备读/写/flush/erase 的包装——discovery 场景测试断言
/// `write_count() == 0`（蓝图 39 WP-39.2：探测快照记录所有 read LBA）。
pub struct BlockTrace<D: BlockDevice> {
    inner: D,
    reads: core::cell::RefCell<Vec<u64>>,
    writes: core::cell::RefCell<Vec<u64>>,
    flushes: core::cell::Cell<u64>,
    erases: core::cell::RefCell<Vec<u64>>,
}

impl<D: BlockDevice> BlockTrace<D> {
    pub fn new(inner: D) -> Self {
        Self {
            inner,
            reads: core::cell::RefCell::new(Vec::new()),
            writes: core::cell::RefCell::new(Vec::new()),
            flushes: core::cell::Cell::new(0),
            erases: core::cell::RefCell::new(Vec::new()),
        }
    }

    pub fn read_count(&self) -> usize {
        self.reads.borrow().len()
    }

    pub fn write_count(&self) -> usize {
        self.writes.borrow().len()
    }

    pub fn flush_count(&self) -> u64 {
        self.flushes.get()
    }

    pub fn erase_count(&self) -> usize {
        self.erases.borrow().len()
    }

    /// 全部写过的 LBA（按序）。
    pub fn writes(&self) -> alloc::vec::Vec<u64> {
        self.writes.borrow().clone()
    }

    /// 全部读过的 LBA（按序）。
    pub fn reads(&self) -> alloc::vec::Vec<u64> {
        self.reads.borrow().clone()
    }

    pub fn into_inner(self) -> D {
        self.inner
    }
}

impl<D: BlockDevice> BlockDevice for BlockTrace<D> {
    fn read_block(&self, block_idx: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        self.reads.borrow_mut().push(block_idx);
        self.inner.read_block(block_idx, buf)
    }

    fn write_block(&mut self, block_idx: u64, buf: &[u8]) -> Result<(), StorageError> {
        self.writes.borrow_mut().push(block_idx);
        self.inner.write_block(block_idx, buf)
    }

    fn erase_block(&mut self, block_idx: u64) -> Result<(), StorageError> {
        self.erases.borrow_mut().push(block_idx);
        self.inner.erase_block(block_idx)
    }

    fn block_count(&self) -> u64 {
        self.inner.block_count()
    }

    fn block_size(&self) -> usize {
        self.inner.block_size()
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        self.flushes.set(self.flushes.get() + 1);
        self.inner.flush()
    }

    fn health_status(&self) -> crate::driver::DeviceHealth {
        self.inner.health_status()
    }
}

// 作用域透传：包裹 scoped 设备的 trace 仍是同一角色的 scoped 设备
//（零写断言不改变安全边界）。
impl<D: ScopedBlockDevice> scope_sealed::Sealed for BlockTrace<D> {}

impl<D: ScopedBlockDevice> ScopedBlockDevice for BlockTrace<D> {
    fn scope_role(&self) -> BlockRole {
        self.inner.scope_role()
    }
}

// ============================================================================
// Provisioning（WP-39.3：显式授权写）
// ============================================================================

/// profile 类别（provisioning capability 的门）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileClass {
    /// 工厂产线 profile——持有 `ProvisionBlock` capability。
    Factory,
    /// 恢复 profile——持有 `ProvisionBlock` capability。
    Recovery,
    /// 正常发布 profile——**不持有** provisioning capability（默认拒绝）。
    Normal,
}

impl ProfileClass {
    pub fn name(&self) -> &'static str {
        match self {
            ProfileClass::Factory => "factory",
            ProfileClass::Recovery => "recovery",
            ProfileClass::Normal => "normal",
        }
    }

    /// 是否允许格式化授权。
    pub const fn allows_provisioning(&self) -> bool {
        !matches!(self, ProfileClass::Normal)
    }
}

/// 格式化请求（目标身份 + 角色 + 容量范围 + nonce + 过期时间）。
#[derive(Debug, Clone, PartialEq)]
pub struct ProvisionRequest {
    /// 目标稳定路径（与发现身份比对，防格式化错盘）。
    pub target_stable_path: [u8; 64],
    /// `target_stable_path` 有效字节数。
    pub target_stable_path_len: u8,
    /// 目标分区角色。
    pub role: BlockRole,
    /// 最小容量（字节）。
    pub min_capacity_bytes: u64,
    /// 一次性授权 nonce（非零）。
    pub nonce: [u8; 16],
    /// 授权过期（Unix 秒）。
    pub expires_at_unix_s: u64,
}

impl ProvisionRequest {
    pub fn target_path_str(&self) -> &str {
        core::str::from_utf8(&self.target_stable_path[..self.target_stable_path_len as usize])
            .unwrap_or("")
    }
}

/// 格式化授权（**只能**经 [`ProvisioningGate::authorize`] 构造——字段私有；
/// 不可 Clone——授权是一次性 capability，重放必须重新授权，nonce 由
/// gate 消费登记）。
#[derive(Debug, PartialEq)]
pub struct ProvisionAuthorization {
    request: ProvisionRequest,
    profile_class: ProfileClass,
    granted_at_unix_s: u64,
    /// 签名 provisioning manifest 的摘要（非零）。
    manifest_digest: [u8; 32],
}

impl ProvisionAuthorization {
    pub fn request(&self) -> &ProvisionRequest {
        &self.request
    }

    pub fn profile_class(&self) -> ProfileClass {
        self.profile_class
    }

    pub fn granted_at_unix_s(&self) -> u64 {
        self.granted_at_unix_s
    }

    pub fn manifest_digest(&self) -> &[u8; 32] {
        &self.manifest_digest
    }
}

/// provisioning 授权错误（全部 fail-closed）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionError {
    /// normal release profile 无 provisioning capability。
    ReleaseProfileDenied,
    /// 人工/产线 policy 未确认（WP-39.3 条件⑤）。
    PolicyNotAcknowledged,
    /// 授权已过期。
    Expired,
    /// nonce 全零（拒绝）。
    ZeroNonce,
    /// nonce 已被消费（重放拒绝——一次性授权）。
    NonceReplay,
    /// manifest 摘要全零（未签名/空摘要）。
    ZeroManifestDigest,
    /// 目标路径与发现身份不符。
    TargetMismatch,
    /// 操作前再读 identity 与首读不符（TOCTOU——WP-39.3 条件⑥）。
    IdentityChanged,
    /// 设备容量低于请求最小值。
    CapacityBelowMinimum,
}

/// provisioning 审计事件（WP-39.3 条件⑦：授权/拒绝均入审计）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionAuditEvent {
    /// 是否授权成功（false = 拒绝）。
    pub granted: bool,
    /// 目标角色。
    pub role: BlockRole,
    /// 请求 nonce（审计可追溯；拒绝事件同样记录）。
    pub nonce: [u8; 16],
    /// 拒绝原因（granted=true 时为 None）。
    pub denied_reason: Option<ProvisionError>,
    /// 授权时刻（Unix 秒）。
    pub at_unix_s: u64,
}

/// provisioning 门（每 boot 由 profile 类别 + 时钟构造）。
///
/// 39 收尾复验：gate 持内部状态——已消费 nonce 集（防重放）与
/// 审计事件序列（条件⑦）；不再 Copy/Clone。
#[derive(Debug)]
pub struct ProvisioningGate {
    profile_class: ProfileClass,
    now_unix_s: u64,
    consumed_nonces: core::cell::RefCell<Vec<[u8; 16]>>,
    audit: core::cell::RefCell<Vec<ProvisionAuditEvent>>,
}

impl ProvisioningGate {
    pub fn new(profile_class: ProfileClass, now_unix_s: u64) -> Self {
        Self {
            profile_class,
            now_unix_s,
            consumed_nonces: core::cell::RefCell::new(Vec::new()),
            audit: core::cell::RefCell::new(Vec::new()),
        }
    }

    pub fn profile_class(&self) -> ProfileClass {
        self.profile_class
    }

    /// 审计事件序列（授权与拒绝均记录——WP-39.3 条件⑦）。
    pub fn audit_log(&self) -> Vec<ProvisionAuditEvent> {
        self.audit.borrow().clone()
    }

    fn record_denied(&self, request: &ProvisionRequest, reason: ProvisionError) {
        self.audit.borrow_mut().push(ProvisionAuditEvent {
            granted: false,
            role: request.role,
            nonce: request.nonce,
            denied_reason: Some(reason),
            at_unix_s: self.now_unix_s,
        });
    }

    /// 授权一次格式化。WP-39.3 条件全部满足才产出 capability：
    /// ① profile 非 Normal；⑤ `operator_ack`（人工/产线 policy 确认）；
    /// ② 未过期；nonce 非零且未被消费（一次性）；manifest 摘要非零；
    /// ③ 目标路径匹配；⑥ `reread_identity` 与首读一致（TOCTOU）；
    /// 容量满足。每次裁决（含拒绝）入审计（⑦）；格式化本体与
    /// 操作后 readback（⑧）由消费方（如 durable-log `provision_new`）
    /// 在执行点完成。
    pub fn authorize(
        &self,
        request: &ProvisionRequest,
        manifest_digest: &[u8; 32],
        identity: &BlockIdentity,
        reread_identity: &BlockIdentity,
        operator_ack: bool,
    ) -> Result<ProvisionAuthorization, ProvisionError> {
        let deny = |reason: ProvisionError| {
            self.record_denied(request, reason);
            Err(reason)
        };
        if !self.profile_class.allows_provisioning() {
            return deny(ProvisionError::ReleaseProfileDenied);
        }
        if !operator_ack {
            return deny(ProvisionError::PolicyNotAcknowledged);
        }
        if self.now_unix_s > request.expires_at_unix_s {
            return deny(ProvisionError::Expired);
        }
        if request.nonce == [0u8; 16] {
            return deny(ProvisionError::ZeroNonce);
        }
        if self.consumed_nonces.borrow().contains(&request.nonce) {
            return deny(ProvisionError::NonceReplay);
        }
        if *manifest_digest == [0u8; 32] {
            return deny(ProvisionError::ZeroManifestDigest);
        }
        let req_path = &request.target_stable_path[..request.target_stable_path_len as usize];
        let id_path = &identity.stable_path[..identity.stable_path_len as usize];
        if req_path != id_path {
            return deny(ProvisionError::TargetMismatch);
        }
        // 条件⑥：操作前再读 identity——稳定路径与容量必须与首读一致
        // （防止授权后换盘/容量变化）。
        let reread_path = &reread_identity.stable_path[..reread_identity.stable_path_len as usize];
        if reread_path != id_path || reread_identity.capacity_bytes != identity.capacity_bytes {
            return deny(ProvisionError::IdentityChanged);
        }
        if identity.capacity_bytes < request.min_capacity_bytes {
            return deny(ProvisionError::CapacityBelowMinimum);
        }
        // nonce 一次性消费（同 gate 生命周期内不可重放）。
        self.consumed_nonces.borrow_mut().push(request.nonce);
        self.audit.borrow_mut().push(ProvisionAuditEvent {
            granted: true,
            role: request.role,
            nonce: request.nonce,
            denied_reason: None,
            at_unix_s: self.now_unix_s,
        });
        Ok(ProvisionAuthorization {
            request: request.clone(),
            profile_class: self.profile_class,
            granted_at_unix_s: self.now_unix_s,
            manifest_digest: *manifest_digest,
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::MockBlockDevice;

    fn mk_dev(blocks: u64, block_size: usize) -> MockBlockDevice {
        MockBlockDevice::new(blocks, block_size)
    }

    /// 写入一个最小合法 GPT（用 image-format 的 writer 生成，避免手拼）。
    /// 分区位于 2048/4096 扇区起（可用范围 34..last_usable 内）。
    fn gpt_image(sector_size: u32, total_sectors: u64) -> Vec<u8> {
        use eneros_image_format::gpt::{write_gpt, Guid, PartitionSpec};
        let data_guid = Guid::parse("454e4552-0009-4001-8000-000000000009").unwrap();
        let data_uniq = Guid::parse("11111111-2222-4333-8444-555555555555").unwrap();
        let audit_guid = Guid::parse("454e4552-000a-4001-8000-00000000000a").unwrap();
        let audit_uniq = Guid::parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap();
        let mut image = alloc::vec![0u8; total_sectors as usize * sector_size as usize];
        write_gpt(
            &mut image,
            sector_size,
            Guid::parse("44444444-5555-4666-8777-888888888888").unwrap(),
            &[
                PartitionSpec {
                    type_guid: data_guid,
                    unique_guid: data_uniq,
                    first_lba: 2048,
                    last_lba: 4095,
                    attributes: 0,
                    name: "DATA",
                },
                PartitionSpec {
                    type_guid: audit_guid,
                    unique_guid: audit_uniq,
                    first_lba: 4096,
                    last_lba: 6143,
                    attributes: 0,
                    name: "LOGS",
                },
            ],
        )
        .expect("write_gpt");
        image
    }

    #[test]
    fn lba_range_checked_translation() {
        let r = LbaRange::new(100, 10).unwrap();
        assert_eq!(r.translate(0), Some(100));
        assert_eq!(r.translate(9), Some(109));
        assert_eq!(r.translate(10), None);
        assert!(r.contains(100) && r.contains(109) && !r.contains(110));
        assert_eq!(LbaRange::new(u64::MAX, 2), None);
    }

    #[test]
    fn lba_range_overlap_detection() {
        let a = LbaRange::new(0, 100).unwrap();
        let b = LbaRange::new(50, 100).unwrap();
        let c = LbaRange::new(100, 10).unwrap();
        assert!(a.overlaps(&b));
        assert!(!a.overlaps(&c)); // 相邻不重叠
        assert!(b.overlaps(&c));
    }

    #[test]
    fn role_typestate_maps_to_role() {
        assert_eq!(DataRole::ROLE, BlockRole::Data);
        assert_eq!(AuditRole::ROLE, BlockRole::Audit);
        assert_eq!(BlockRole::Data.name(), "data");
    }

    #[test]
    fn default_policy_matches_qemu_layout_guids() {
        let p = RolePolicy::default();
        assert_eq!(
            p.lookup(&Guid::parse("454e4552-0009-4001-8000-000000000009").unwrap()),
            Some(BlockRole::Data)
        );
        assert_eq!(
            p.lookup(&Guid::parse("454e4552-000a-4001-8000-00000000000a").unwrap()),
            Some(BlockRole::Audit)
        );
        assert_eq!(
            p.lookup(&Guid::parse("c12a7328-f81f-11d2-ba4b-00a0c93ec93b").unwrap()),
            Some(BlockRole::Boot)
        );
        // BOOTCTRL 不映射
        assert_eq!(
            p.lookup(&Guid::parse("454e4552-0002-4001-8000-000000000002").unwrap()),
            None
        );
    }

    #[test]
    #[allow(clippy::explicit_counter_loop)] // 44.2：GPT 分块注入人工计数（chunk 语义明确）
    fn resolver_parses_gpt_and_matches_roles() {
        // 10000 扇区 × 512B：可用范围 34..=9966，分区 2048..4095 / 4096..6143。
        let mut dev = mk_dev(10000, 512);
        let img = gpt_image(512, 10000);
        let mut idx = 0u64;
        for chunk in img.chunks(512) {
            if idx >= 10000 {
                break;
            }
            dev.write_block(idx, chunk).unwrap();
            idx += 1;
        }
        let policy = RolePolicy::default();
        let identities = GptResolver::resolve(&mut dev, &policy).unwrap();
        assert_eq!(identities.len(), 2);
        let data = identities
            .iter()
            .find(|p| p.role == BlockRole::Data)
            .unwrap();
        assert_eq!(data.range.start_lba, 2048);
        assert_eq!(data.range.block_count, 4096 - 2048);
        assert_eq!(data.name_str(), "DATA");
    }

    #[test]
    #[allow(clippy::explicit_counter_loop)] // 44.2：GPT 分块注入人工计数（chunk 语义明确）
    fn resolver_reads_4kn_device_via_sector_adapter() {
        // 4Kn 设备（块大小 4096）：GPT 逻辑扇区 512B，适配层按块读+切片。
        // 2000 块 × 4096 = 8000 扇区；分区 2048..4095 / 4096..6143。
        let mut dev = mk_dev(2000, 4096);
        let img = gpt_image(512, 16000);
        let mut idx = 0u64;
        for chunk in img.chunks(4096) {
            if idx >= 2000 {
                break;
            }
            dev.write_block(idx, chunk).unwrap();
            idx += 1;
        }
        let policy = RolePolicy::default();
        let identities = GptResolver::resolve(&mut dev, &policy).unwrap();
        assert_eq!(identities.len(), 2);
        let audit = identities
            .iter()
            .find(|p| p.role == BlockRole::Audit)
            .unwrap();
        assert_eq!(audit.range.start_lba, 4096);
        assert_eq!(audit.range.block_count, 6144 - 4096);
    }

    #[test]
    #[allow(clippy::explicit_counter_loop)] // 44.2：GPT 分块注入人工计数（chunk 语义明确）
    fn resolver_zero_writes_during_discovery() {
        let mut dev = BlockTrace::new(mk_dev(10000, 512));
        let img = gpt_image(512, 10000);
        let mut idx = 0u64;
        for chunk in img.chunks(512) {
            if idx >= 10000 {
                break;
            }
            dev.write_block(idx, chunk).unwrap();
            idx += 1;
        }
        // 设置阶段有写（铺 GPT）；解析阶段必须零写。
        let before = dev.write_count();
        let policy = RolePolicy::default();
        let identities = GptResolver::resolve(&mut dev, &policy).unwrap();
        assert!(!identities.is_empty());
        assert_eq!(dev.write_count(), before, "GPT 解析必须零写");
        assert_eq!(dev.flush_count(), 0);
        assert_eq!(dev.erase_count(), 0);
    }

    #[test]
    fn resolver_rejects_duplicate_guids_and_overlap() {
        // 39 收尾复验（替换旧弱断言）：真构造重复 unique GUID 与重叠
        // 分区的 GPT 镜像（primary/backup 双份同步 patch + CRC 重算），
        // 断言 resolver 返回**具体**拒绝错误而非泛化 NoGpt。
        // —— 重复 unique GUID：第二条目 unique GUID 改为与第一条相同。
        let mut img = gpt_image(512, 10000);
        let (pe, be, _ph, _bh) = gpt_layout(512, 10000);
        for table in [pe, be] {
            let first: [u8; 16] = img[table + 16..table + 32].try_into().unwrap();
            img[table + 128 + 16..table + 128 + 32].copy_from_slice(&first);
        }
        recompute_gpt_crcs(&mut img, 512, 10000);
        let mut dev = mk_dev(10000, 512);
        load_image(&mut dev, &img);
        assert_eq!(
            GptResolver::resolve(&mut dev, &RolePolicy::default()),
            Err(PartitionResolveError::DuplicateGuid)
        );

        // —— 重叠：第二条目 first_lba 改入第一条范围（2048..4095）。
        let mut img = gpt_image(512, 10000);
        let (pe, be, _, _) = gpt_layout(512, 10000);
        for table in [pe, be] {
            img[table + 128 + 32..table + 128 + 40].copy_from_slice(&3000u64.to_le_bytes());
        }
        recompute_gpt_crcs(&mut img, 512, 10000);
        let mut dev = mk_dev(10000, 512);
        load_image(&mut dev, &img);
        assert_eq!(
            GptResolver::resolve(&mut dev, &RolePolicy::default()),
            Err(PartitionResolveError::Overlap)
        );
    }

    /// GPT 布局偏移（与 image-format Geometry 一致：primary entries=LBA2，
    /// backup header=末扇区，backup entries=末扇区前 32 扇区（512B））。
    fn gpt_layout(sector_size: usize, total_sectors: u64) -> (usize, usize, usize, usize) {
        let entry_bytes = 128 * 128usize;
        let entry_sectors = entry_bytes / sector_size;
        let primary_entries = 2 * sector_size;
        let backup_header = (total_sectors - 1) as usize * sector_size;
        let backup_entries = (total_sectors - 1 - entry_sectors as u64) as usize * sector_size;
        (primary_entries, backup_entries, sector_size, backup_header)
    }

    /// patch 条目后重算双份 GPT 的 CRC（entries CRC → 两头 [88..92]；
    /// header CRC：[16..20] 置零后对前 92 字节）。
    fn recompute_gpt_crcs(img: &mut [u8], sector_size: usize, total_sectors: u64) {
        let entry_bytes = 128 * 128usize;
        let (pe, _be, _ph, bh) = gpt_layout(sector_size, total_sectors);
        let entries_crc = crate::crc32::crc32(&img[pe..pe + entry_bytes]);
        for hdr in [sector_size, bh] {
            img[hdr + 88..hdr + 92].copy_from_slice(&entries_crc.to_le_bytes());
            img[hdr + 16..hdr + 20].copy_from_slice(&0u32.to_le_bytes());
            let header_crc = crate::crc32::crc32(&img[hdr..hdr + 92]);
            img[hdr + 16..hdr + 20].copy_from_slice(&header_crc.to_le_bytes());
        }
    }

    /// 镜像逐块写入设备。
    fn load_image(dev: &mut MockBlockDevice, img: &[u8]) {
        let bs = dev.block_size();
        for (idx, chunk) in img.chunks(bs).enumerate() {
            if idx as u64 >= dev.block_count() {
                break;
            }
            dev.write_block(idx as u64, chunk).unwrap();
        }
    }

    #[test]
    fn resolver_rejects_capacity_zero() {
        let mut dev = mk_dev(0, 512);
        assert_eq!(
            GptResolver::resolve(&mut dev, &RolePolicy::default()),
            Err(PartitionResolveError::CapacityZero)
        );
    }

    #[test]
    #[allow(clippy::explicit_counter_loop)] // 44.2：GPT 分块注入人工计数（chunk 语义明确）
    fn partition_handle_read_write_stays_in_slice() {
        let mut dev = mk_dev(10000, 512);
        let img = gpt_image(512, 10000);
        let mut idx = 0u64;
        for chunk in img.chunks(512) {
            if idx >= 10000 {
                break;
            }
            dev.write_block(idx, chunk).unwrap();
            idx += 1;
        }
        let policy = RolePolicy::default();
        let mut handle: PartitionHandle<DataRole, _> = GptResolver::open(dev, &policy).unwrap();
        assert_eq!(handle.block_count(), 4096 - 2048);
        assert_eq!(handle.range().start_lba, 2048);
        // 分区内读写
        let mut buf = [0u8; 512];
        handle.write_block(0, &buf).unwrap();
        handle.read_block(0, &mut buf).unwrap();
        // 越界 → OutOfRange（不写坏盘）
        assert_eq!(
            handle.write_block(handle.block_count(), &buf),
            Err(StorageError::OutOfRange {
                block_idx: handle.block_count(),
                max: handle.block_count()
            })
        );
    }

    #[test]
    fn provisioning_gate_normal_profile_denied() {
        let gate = ProvisioningGate::new(ProfileClass::Normal, 1000);
        let mut req = ProvisionRequest {
            target_stable_path: [0u8; 64],
            target_stable_path_len: 4,
            role: BlockRole::Data,
            min_capacity_bytes: 1024,
            nonce: [1; 16],
            expires_at_unix_s: 2000,
        };
        req.target_stable_path[..4].copy_from_slice(b"nvme");
        let identity = BlockIdentity {
            device_id: 1,
            stable_path: [0u8; 64],
            stable_path_len: 4,
            capacity_bytes: 4096,
            logical_sector: 512,
            physical_sector: 512,
            gpt_disk_guid: None,
            media_serial_hash: None,
        };
        let mut id2 = identity.clone();
        id2.stable_path[..4].copy_from_slice(b"nvme");
        id2.stable_path_len = 4;
        assert_eq!(
            gate.authorize(&req, &[7u8; 32], &id2, &id2, true),
            Err(ProvisionError::ReleaseProfileDenied)
        );
    }

    #[test]
    fn provisioning_gate_factory_grants_and_validates() {
        let gate = ProvisioningGate::new(ProfileClass::Factory, 1000);
        let mut req = ProvisionRequest {
            target_stable_path: [0u8; 64],
            target_stable_path_len: 4,
            role: BlockRole::Data,
            min_capacity_bytes: 1024,
            nonce: [1; 16],
            expires_at_unix_s: 2000,
        };
        req.target_stable_path[..4].copy_from_slice(b"nvme");
        let mut identity = BlockIdentity {
            device_id: 1,
            stable_path: [0u8; 64],
            stable_path_len: 4,
            capacity_bytes: 4096,
            logical_sector: 512,
            physical_sector: 512,
            gpt_disk_guid: None,
            media_serial_hash: None,
        };
        identity.stable_path[..4].copy_from_slice(b"nvme");
        identity.stable_path_len = 4;

        let auth = gate
            .authorize(&req, &[7u8; 32], &identity, &identity, true)
            .expect("factory authorize");
        assert_eq!(auth.profile_class(), ProfileClass::Factory);
        assert_eq!(auth.granted_at_unix_s(), 1000);

        // 39 收尾复验：nonce 一次性消费——同一 nonce 再次授权 → NonceReplay。
        assert_eq!(
            gate.authorize(&req, &[7u8; 32], &identity, &identity, true),
            Err(ProvisionError::NonceReplay)
        );
        // policy 未确认（条件⑤）。
        let mut req2 = req.clone();
        req2.nonce = [2; 16];
        assert_eq!(
            gate.authorize(&req2, &[7u8; 32], &identity, &identity, false),
            Err(ProvisionError::PolicyNotAcknowledged)
        );
        // 再读 identity 不符（条件⑥ TOCTOU：容量变化）。
        let mut reread = identity.clone();
        reread.capacity_bytes = 8192;
        let mut req3 = req.clone();
        req3.nonce = [3; 16];
        assert_eq!(
            gate.authorize(&req3, &[7u8; 32], &identity, &reread, true),
            Err(ProvisionError::IdentityChanged)
        );
        // 审计（条件⑦）：每次裁决（含拒绝）均入序列。
        let audit = gate.audit_log();
        assert_eq!(audit.len(), 4);
        assert!(audit[0].granted && audit[0].denied_reason.is_none());
        assert_eq!(audit[1].denied_reason, Some(ProvisionError::NonceReplay));
        assert_eq!(
            audit[2].denied_reason,
            Some(ProvisionError::PolicyNotAcknowledged)
        );
        assert_eq!(
            audit[3].denied_reason,
            Some(ProvisionError::IdentityChanged)
        );

        // 过期
        let expired = ProvisioningGate::new(ProfileClass::Factory, 3000);
        assert_eq!(
            expired.authorize(&req, &[7u8; 32], &identity, &identity, true),
            Err(ProvisionError::Expired)
        );
        // 目标不符（新 nonce——[1;16] 已被首次授权消费）
        let mut wrong = identity.clone();
        wrong.stable_path[..4].copy_from_slice(b"mmc!");
        wrong.stable_path_len = 4;
        let mut req_wrong = req.clone();
        req_wrong.nonce = [4; 16];
        assert_eq!(
            gate.authorize(&req_wrong, &[7u8; 32], &wrong, &wrong, true),
            Err(ProvisionError::TargetMismatch)
        );
        // 容量不足
        let mut small = identity.clone();
        small.capacity_bytes = 512;
        let mut req_small = req.clone();
        req_small.nonce = [5; 16];
        assert_eq!(
            gate.authorize(&req_small, &[7u8; 32], &small, &small, true),
            Err(ProvisionError::CapacityBelowMinimum)
        );
        // 零 nonce / 零 digest
        let mut zero_nonce = req.clone();
        zero_nonce.nonce = [0; 16];
        assert_eq!(
            gate.authorize(&zero_nonce, &[7u8; 32], &identity, &identity, true),
            Err(ProvisionError::ZeroNonce)
        );
        let mut req_zd = req.clone();
        req_zd.nonce = [6; 16];
        assert_eq!(
            gate.authorize(&req_zd, &[0u8; 32], &identity, &identity, true),
            Err(ProvisionError::ZeroManifestDigest)
        );
    }
}
