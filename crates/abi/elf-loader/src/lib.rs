//! ELF64 loader（共享 no_std crate，KRN-002）。
//!
//! 由 agent（AGT-003 包装载）与 EnerOS root-task（EL0 服务装载，KRN-002/003）
//! 共用：解析 ELF header、遍历 program headers、提取 PT_LOAD 段，并强制：
//! - ELF64 + AArch64（拒绝 ELF32 / 非 AArch64）；
//! - 可执行格式：ET_DYN（PIE，agent 场景）或 ET_EXEC（固定地址，
//!   aarch64-unknown-none 裸机 EL0 服务与 rootserver——C2/BOOT-007 修复后
//!   root-task 经 plan_spawn 解析自身 ET_EXEC ELF，原 PIE-only 校验导致
//!   BadElf）；
//! - W^X（禁止可写且可执行段）；
//! - 有界（所有 offset/size 相对输入缓冲校验）；
//! - 不跨二进制的 Rust ABI（返回 LoadedElf 段描述符，而非 trait 对象）。
//!
//! 来源：agent/src/elf.rs（KRN-001 三要素：ELF loader 复用），agent 侧保持
//! `agent::elf` 路径 re-export 兼容。

#![no_std]

extern crate alloc;

#[cfg(test)]
use alloc::vec;
use alloc::vec::Vec;
use core::convert::TryInto;

// ============================================================================
// ELF constants
// ============================================================================

/// ELF magic: 0x7f 'E' 'L' 'F'
pub const ELFMAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// ELF class: 2 = ELF64
pub const ELFCLASS64: u8 = 2;

/// ELF data: 1 = little-endian
pub const ELFDATA2LSB: u8 = 1;

/// ELF type: 3 = ET_DYN (shared object / PIE)
pub const ET_DYN: u16 = 3;

/// ELF type: 2 = ET_EXEC (fixed-address executable, bare-metal EL0)
pub const ET_EXEC: u16 = 2;

/// ELF machine: 183 = EM_AARCH64
pub const EM_AARCH64: u16 = 183;

/// Program header type: PT_LOAD
pub const PT_LOAD: u32 = 1;
/// Program header type: PT_DYNAMIC（30 WP-27.7 重定位用）。
pub const PT_DYNAMIC: u32 = 2;
/// Dynamic tag: DT_NULL（.dynamic 表终止符）。
pub const DT_NULL: u64 = 0;
/// Dynamic tag: DT_RELA（重定位表文件偏移）。
pub const DT_RELA: u64 = 7;
/// Dynamic tag: DT_RELASZ（重定位表字节数）。
pub const DT_RELASZ: u64 = 8;
/// Dynamic tag: DT_RELAENT（单条重定位大小）。
pub const DT_RELAENT: u64 = 9;
/// AArch64 重定位类型：R_AARCH64_RELATIVE（`*(base + r_offset) = base + r_addend`）。
pub const R_AARCH64_RELATIVE: u64 = 1027;
/// 重定位条目大小（Elf64_Rela：r_offset(8) + r_info(8) + r_addend(8)）。
pub const ELF64_RELA_SIZE: u64 = 24;

/// Program header flags
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

/// ELF64 header size
pub const ELF64_EHDR_SIZE: usize = 64;

/// ELF64 program header entry size
pub const ELF64_PHDR_SIZE: usize = 56;

// ============================================================================
// ELF types
// ============================================================================

/// Segment permissions (R/W/X flags from PT_LOAD).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentFlags {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl SegmentFlags {
    pub fn from_p_flags(p_flags: u32) -> Self {
        Self {
            read: p_flags & PF_R != 0,
            write: p_flags & PF_W != 0,
            execute: p_flags & PF_X != 0,
        }
    }

    /// W^X violation: segment is both writable and executable.
    pub fn is_wx_violation(self) -> bool {
        self.write && self.execute
    }
}

/// A loaded PT_LOAD segment descriptor.
#[derive(Debug, Clone, Copy)]
pub struct LoadedSegment {
    /// Virtual address where the segment should be mapped.
    pub vaddr: u64,
    /// Size in memory (may be larger than file size due to BSS).
    pub memsz: u64,
    /// Size in file.
    pub filesz: u64,
    /// Offset in the ELF file.
    pub offset: u64,
    /// Segment permissions.
    pub flags: SegmentFlags,
    /// Alignment.
    pub align: u64,
}

/// 30 WP-27.7：动态段信息（重定位输入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DynamicInfo {
    /// DT_RELA：重定位表文件偏移。
    pub rela_offset: u64,
    /// DT_RELASZ：重定位表字节数。
    pub rela_size: u64,
    /// DT_RELAENT：单条重定位大小（缺省 = 24B）。
    pub rela_ent: u64,
}

/// WP-37.4：ELF 装载策略（唯一装载决策入口的参数）。
///
/// - `allow_et_exec`：ET_EXEC（固定链接地址）是否允许。release 若冻结
///   为 static PIE 只允许 ET_DYN，应置 false；
/// - `dyn_bias`：ET_DYN 装载基址（页对齐，进程可寻址区域内）。装载后
///   虚拟地址 = `bias + 段链接 vaddr`；同 ELF 不同 bias = 不同装载实例。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElfLoadPolicy {
    /// 是否允许 ET_EXEC（固定地址可执行文件）。
    pub allow_et_exec: bool,
    /// ET_DYN 装载基址（bias）。
    pub dyn_bias: u64,
}

/// WP-37.4：装载结果中的单个段布局（未映射，纯地址/权限事实）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadedImageSegment {
    /// 装载后最终虚拟地址（ET_DYN：`bias + vaddr`；ET_EXEC：`vaddr`）。
    pub vaddr: u64,
    /// 内存大小（含 BSS 清零区）。
    pub memsz: u64,
    /// 文件数据大小。
    pub filesz: u64,
    /// 段文件数据在 ELF 镜像中的偏移。
    pub file_offset: u64,
    /// 最终权限（text=R-X / rodata=R-- / data=RW-）。
    pub permissions: SegmentFlags,
}

/// WP-37.4：统一装载结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedImage {
    /// 装载后入口虚拟地址（bias 平移后）。
    pub entry: u64,
    /// 段装载计划（按装载后 vaddr 升序）。
    pub segments: Vec<LoadedImageSegment>,
    /// ET_DYN 且含 DT_RELA → 需要（已完成 staging 内）重定位。
    pub needs_relocation: bool,
    /// 重定位写入过可执行段 → 映射后需要 I-cache/D-cache 一致性维护。
    pub icache_after_reloc: bool,
}

/// WP-37.4：AArch64 static PIE 允许的重定位白名单。
/// 当前冻结：`R_AARCH64_RELATIVE`（基址相对）。其余类型一律拒绝
/// （fail-closed；如 release 需要 ABS64 等，先扩展白名单再放行）。
pub fn is_allowed_relocation(r_type: u64) -> bool {
    r_type == R_AARCH64_RELATIVE
}

/// Parsed and validated ELF64 binary.
#[derive(Debug)]
pub struct LoadedElf {
    /// ELF 类型（ET_DYN=3 / ET_EXEC=2；WP-37.4 接线：统一 loader 分流依据）。
    e_type: u16,
    /// Entry point virtual address.
    entry: u64,
    /// All PT_LOAD segments.
    segments: Vec<LoadedSegment>,
}

/// ELF parse error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElfError {
    /// Input too short for ELF header.
    TruncatedHeader,
    /// Bad ELF magic bytes.
    BadMagic,
    /// Not ELF64 (wrong class).
    NotElf64,
    /// Not little-endian.
    NotLittleEndian,
    /// Not a PIE (ET_DYN).
    NotPie,
    /// Not AArch64.
    WrongMachine,
    /// Program header table out of bounds.
    PhdrOutOfBounds,
    /// PT_LOAD segment offset+filesz exceeds file size.
    SegmentOutOfBounds,
    /// W^X violation: a segment is both writable and executable.
    WxViolation,
    /// No PT_LOAD segments (empty binary).
    NoLoadSegments,
    /// Entry point not within any executable segment.
    EntryNotInExecutableSegment,
    /// 30 RT-006：PT_LOAD 段 vaddr 区间重叠（或段边界回绕）——非单调段
    /// 导致同一虚拟页两次映射/与栈区碰撞。
    SegmentOverlap,
    /// 30 WP-27.7：PT_DYNAMIC 缺失/损坏（重定位前必须存在）。
    BadDynamic,
    /// 30 WP-27.7：重定位表缺失/异常（DT_RELA/DT_RELASZ/DT_RELAENT 非法）。
    BadRelaTable,
    /// 30 WP-27.7：重定位写入目标越界（不在任何可写段内——恶意 Rela 防
    /// 越界写）。
    RelocOutOfBounds,
    /// 30 WP-27.7：不支持的 Rela 类型（fail-closed，不静默跳过）。
    UnsupportedReloc,
    /// WP-37.4：ET_EXEC 被装载策略拒绝（release 冻结 static PIE 时）。
    PolicyRejectsExec,
    /// WP-37.4：ET_DYN 装载基址非页对齐。
    BiasMisaligned,
    /// WP-37.4：调用方提供的 staging 缓冲小于装载跨度。
    StagingTooSmall,
}

impl core::fmt::Display for ElfError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ElfError::TruncatedHeader => write!(f, "ELF header truncated"),
            ElfError::BadMagic => write!(f, "bad ELF magic"),
            ElfError::NotElf64 => write!(f, "not ELF64"),
            ElfError::NotLittleEndian => write!(f, "not little-endian"),
            ElfError::NotPie => write!(f, "not PIE (ET_DYN required)"),
            ElfError::WrongMachine => write!(f, "not AArch64"),
            ElfError::PhdrOutOfBounds => write!(f, "program header table out of bounds"),
            ElfError::SegmentOutOfBounds => write!(f, "PT_LOAD segment exceeds file bounds"),
            ElfError::WxViolation => write!(f, "W^X violation: segment is W+X"),
            ElfError::NoLoadSegments => write!(f, "no PT_LOAD segments"),
            ElfError::EntryNotInExecutableSegment => {
                write!(f, "entry point not in executable segment")
            }
            ElfError::SegmentOverlap => write!(f, "segment vaddr range overlap or wrap"),
            ElfError::BadDynamic => write!(f, "PT_DYNAMIC missing or corrupt"),
            ElfError::BadRelaTable => write!(f, "relocation table missing or malformed"),
            ElfError::RelocOutOfBounds => {
                write!(f, "relocation write target out of writable segments")
            }
            ElfError::UnsupportedReloc => write!(f, "unsupported relocation type"),
            ElfError::PolicyRejectsExec => write!(f, "ET_EXEC rejected by load policy"),
            ElfError::BiasMisaligned => write!(f, "ET_DYN load bias not page-aligned"),
            ElfError::StagingTooSmall => write!(f, "staging buffer smaller than load span"),
        }
    }
}

// ============================================================================
// ELF parser
// ============================================================================

impl LoadedElf {
    /// Parse and validate an ELF64 PIE binary.
    ///
    /// Performs full validation:
    /// 1. ELF header (magic, class, data, type, machine)
    /// 2. Program header table bounds
    /// 3. PT_LOAD segment bounds
    /// 4. W^X enforcement
    /// 5. Entry point within executable segment
    pub fn parse(data: &[u8]) -> Result<Self, ElfError> {
        if data.len() < ELF64_EHDR_SIZE {
            return Err(ElfError::TruncatedHeader);
        }

        // --- e_ident ---
        if data[0..4] != ELFMAG {
            return Err(ElfError::BadMagic);
        }
        if data[4] != ELFCLASS64 {
            return Err(ElfError::NotElf64);
        }
        if data[5] != ELFDATA2LSB {
            return Err(ElfError::NotLittleEndian);
        }

        // --- ELF header fields (little-endian) ---
        let e_type = u16::from_le_bytes(data[16..18].try_into().unwrap());
        let e_machine = u16::from_le_bytes(data[18..20].try_into().unwrap());
        let e_entry = u64::from_le_bytes(data[24..32].try_into().unwrap());
        let e_phoff = u64::from_le_bytes(data[32..40].try_into().unwrap());
        let e_phentsize = u16::from_le_bytes(data[54..56].try_into().unwrap());
        let e_phnum = u16::from_le_bytes(data[56..58].try_into().unwrap());

        if e_type != ET_DYN && e_type != ET_EXEC {
            return Err(ElfError::NotPie);
        }
        if e_machine != EM_AARCH64 {
            return Err(ElfError::WrongMachine);
        }

        // --- Program header table bounds ---
        let phoff = e_phoff as usize;
        let phentsize = e_phentsize as usize;
        let phnum = e_phnum as usize;

        if phentsize < ELF64_PHDR_SIZE {
            return Err(ElfError::PhdrOutOfBounds);
        }
        let phdr_end = phoff
            .checked_add(
                phentsize
                    .checked_mul(phnum)
                    .ok_or(ElfError::PhdrOutOfBounds)?,
            )
            .ok_or(ElfError::PhdrOutOfBounds)?;
        if phdr_end > data.len() {
            return Err(ElfError::PhdrOutOfBounds);
        }

        // --- Iterate program headers, extract PT_LOAD ---
        let mut segments = Vec::new();
        for i in 0..phnum {
            let off = phoff + i * phentsize;
            let p_type = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
            if p_type != PT_LOAD {
                continue;
            }

            let p_flags = u32::from_le_bytes(data[off + 4..off + 8].try_into().unwrap());
            let p_offset = u64::from_le_bytes(data[off + 8..off + 16].try_into().unwrap());
            let p_vaddr = u64::from_le_bytes(data[off + 16..off + 24].try_into().unwrap());
            let p_filesz = u64::from_le_bytes(data[off + 32..off + 40].try_into().unwrap());
            let p_memsz = u64::from_le_bytes(data[off + 40..off + 48].try_into().unwrap());
            let p_align = u64::from_le_bytes(data[off + 48..off + 56].try_into().unwrap());

            let flags = SegmentFlags::from_p_flags(p_flags);

            // W^X enforcement.
            if flags.is_wx_violation() {
                return Err(ElfError::WxViolation);
            }

            // Bounds check: offset + filesz must fit in the file.
            let seg_end = p_offset
                .checked_add(p_filesz)
                .ok_or(ElfError::SegmentOutOfBounds)?;
            if seg_end > data.len() as u64 {
                return Err(ElfError::SegmentOutOfBounds);
            }

            // memsz >= filesz (BSS = memsz - filesz).
            if p_memsz < p_filesz {
                return Err(ElfError::SegmentOutOfBounds);
            }

            segments.push(LoadedSegment {
                vaddr: p_vaddr,
                memsz: p_memsz,
                filesz: p_filesz,
                offset: p_offset,
                flags,
                align: p_align,
            });
        }

        if segments.is_empty() {
            return Err(ElfError::NoLoadSegments);
        }

        // --- Entry point must be within an executable segment ---
        // 30 RT-005：`vaddr + memsz` 必须 checked_add——恶意段头
        // (vaddr=0xFFFF_FFFF_FFFF_F000, memsz=0x2000) 使和回绕为小数，
        // 入口校验被绕过。
        let entry_in_exec = segments.iter().any(|s| {
            s.flags.execute
                && s.vaddr
                    .checked_add(s.memsz)
                    .is_some_and(|end| e_entry >= s.vaddr && e_entry < end)
        });
        if !entry_in_exec {
            return Err(ElfError::EntryNotInExecutableSegment);
        }

        // 30 RT-006：段区间重叠/回绕校验（落点统一——与 spawn.rs 共用
        // validate_segments 纯函数）。
        Self::validate_segments(&segments)?;

        Ok(Self {
            e_type,
            entry: e_entry,
            segments,
        })
    }

    /// WP-37.4：是否为 ET_DYN（PIE）——统一 loader 分流依据。
    pub fn is_et_dyn(&self) -> bool {
        self.e_type == ET_DYN
    }

    /// Entry point virtual address.
    pub fn entry(&self) -> u64 {
        self.entry
    }

    /// All PT_LOAD segments.
    pub fn segments(&self) -> &[LoadedSegment] {
        &self.segments
    }

    /// 30 RT-006 落点统一：段 vaddr 区间校验纯函数（spawn.rs 复用，禁止
    /// 两处各写）。检查：①每段边界 `vaddr+memsz` 无回绕；②段区间两两不
    /// 重叠（排序后相邻比较）——非单调/重叠段 → 同一虚拟页两次映射或与
    /// 栈区碰撞。
    pub fn validate_segments(segs: &[LoadedSegment]) -> Result<(), ElfError> {
        let mut spans: alloc::vec::Vec<(u64, u64)> = segs
            .iter()
            .map(|s| {
                let end = s
                    .vaddr
                    .checked_add(s.memsz)
                    .ok_or(ElfError::SegmentOverlap)?;
                Ok((s.vaddr, end))
            })
            .collect::<Result<_, ElfError>>()?;
        spans.sort_unstable();
        for pair in spans.windows(2) {
            if pair[0].1 > pair[1].0 {
                return Err(ElfError::SegmentOverlap);
            }
        }
        Ok(())
    }

    /// WP-37.4：镜像是否含 PT_DYNAMIC 段（无 = 静态无重定位 PIE，跳过
    /// 重定位；有但损坏 = parse_dynamic 严格拒绝）。与 parse_dynamic 的
    /// phdr 遍历同构（e_phoff/e_phentsize/e_phnum + 界校验）。
    fn has_pt_dynamic(image: &[u8]) -> Result<bool, ElfError> {
        if image.len() < ELF64_EHDR_SIZE {
            return Err(ElfError::TruncatedHeader);
        }
        let e_phoff = u64::from_le_bytes(image[32..40].try_into().unwrap()) as usize;
        let e_phentsize = u16::from_le_bytes(image[54..56].try_into().unwrap()) as usize;
        let e_phnum = u16::from_le_bytes(image[56..58].try_into().unwrap()) as usize;
        if e_phentsize < ELF64_PHDR_SIZE {
            return Err(ElfError::PhdrOutOfBounds);
        }
        let phdr_end = e_phoff
            .checked_add(
                e_phentsize
                    .checked_mul(e_phnum)
                    .ok_or(ElfError::PhdrOutOfBounds)?,
            )
            .ok_or(ElfError::PhdrOutOfBounds)?;
        if phdr_end > image.len() {
            return Err(ElfError::PhdrOutOfBounds);
        }
        for i in 0..e_phnum {
            let off = e_phoff + i * e_phentsize;
            let p_type = u32::from_le_bytes(image[off..off + 4].try_into().unwrap());
            if p_type == PT_DYNAMIC {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 30 WP-27.7：解析 PT_DYNAMIC 段（重定位输入）——缺失/损坏 → Err。
    ///
    /// `data` = 完整 ELF 文件字节（与 parse 同输入）。
    pub fn parse_dynamic(data: &[u8]) -> Result<DynamicInfo, ElfError> {
        if data.len() < ELF64_EHDR_SIZE {
            return Err(ElfError::TruncatedHeader);
        }
        let e_phoff = u64::from_le_bytes(data[32..40].try_into().unwrap()) as usize;
        let e_phentsize = u16::from_le_bytes(data[54..56].try_into().unwrap()) as usize;
        let e_phnum = u16::from_le_bytes(data[56..58].try_into().unwrap()) as usize;
        if e_phentsize < ELF64_PHDR_SIZE {
            return Err(ElfError::PhdrOutOfBounds);
        }
        let phdr_end = e_phoff
            .checked_add(
                e_phentsize
                    .checked_mul(e_phnum)
                    .ok_or(ElfError::PhdrOutOfBounds)?,
            )
            .ok_or(ElfError::PhdrOutOfBounds)?;
        if phdr_end > data.len() {
            return Err(ElfError::PhdrOutOfBounds);
        }
        let mut dyn_off = None;
        let mut dyn_filesz = 0u64;
        for i in 0..e_phnum {
            let off = e_phoff + i * e_phentsize;
            let p_type = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
            if p_type == PT_DYNAMIC {
                dyn_off = Some(u64::from_le_bytes(
                    data[off + 8..off + 16].try_into().unwrap(),
                ));
                dyn_filesz = u64::from_le_bytes(data[off + 32..off + 40].try_into().unwrap());
            }
        }
        let dyn_off = dyn_off.ok_or(ElfError::BadDynamic)?;
        let dyn_end = dyn_off
            .checked_add(dyn_filesz)
            .ok_or(ElfError::BadDynamic)?;
        if dyn_end > data.len() as u64 {
            return Err(ElfError::BadDynamic);
        }
        // 遍历 .dynamic（Elf64_Dyn：tag(8) + val(8)，DT_NULL 终止）。
        let mut rela_offset: Option<u64> = None;
        let mut rela_size: Option<u64> = None;
        let mut rela_ent: Option<u64> = None;
        let mut cursor = dyn_off as usize;
        let end = dyn_end as usize;
        while cursor + 16 <= end {
            let tag = u64::from_le_bytes(data[cursor..cursor + 8].try_into().unwrap());
            let val = u64::from_le_bytes(data[cursor + 8..cursor + 16].try_into().unwrap());
            if tag == DT_NULL {
                break;
            }
            match tag {
                DT_RELA => rela_offset = Some(val),
                DT_RELASZ => rela_size = Some(val),
                DT_RELAENT => rela_ent = Some(val),
                _ => {}
            }
            cursor += 16;
        }
        Ok(DynamicInfo {
            rela_offset: rela_offset.ok_or(ElfError::BadRelaTable)?,
            rela_size: rela_size.ok_or(ElfError::BadRelaTable)?,
            rela_ent: rela_ent.unwrap_or(ELF64_RELA_SIZE),
        })
    }

    /// 30 WP-27.7：应用重定位（S7 顺序：验签 → 映射 → 重定位 → 跳转）。
    ///
    /// `img` = **链接地址平坦化镜像**（`img[k]` = 虚拟地址 k 处的字节，
    /// 段间空洞由装配层填零；**当前支持形态要求文件偏移 == 链接地址**——
    /// DT_RELA 以文件偏移语义定位、r_offset 以链接地址语义写入，二者在
    /// img 中重合；真实 ELF（vaddr ≠ p_offset）重定位须先扩展本函数，
    /// 31 域装配层接线时处理）。
    /// `base` = 加载基址（链接地址偏移）；`segments` = PT_LOAD 段描述。
    ///
    /// 校验（fail-closed）：
    /// - Rela 表边界（rela_ent 合法且 size 整除、r_offset 与 8 字节写入
    ///   目标完全落在某**可写**段内且不越 img 界——防恶意 Rela 越界写
    ///   与切片 panic）；
    /// - 仅支持 `R_AARCH64_RELATIVE`；其他类型 → `Err(UnsupportedReloc)`。
    pub fn apply_relocations(
        img: &mut [u8],
        base: u64,
        dyn_: &DynamicInfo,
        segments: &[LoadedSegment],
    ) -> Result<(), ElfError> {
        if dyn_.rela_ent < ELF64_RELA_SIZE || dyn_.rela_ent % 8 != 0 {
            return Err(ElfError::BadRelaTable);
        }
        // 审查修复：size 必须整除 ent——旧实现余数条目被静默忽略
        // （fail-closed 红线：表截断即拒绝）。
        if dyn_.rela_size % dyn_.rela_ent != 0 {
            return Err(ElfError::BadRelaTable);
        }
        let rela_end = dyn_
            .rela_offset
            .checked_add(dyn_.rela_size)
            .ok_or(ElfError::BadRelaTable)?;
        if rela_end > img.len() as u64 {
            return Err(ElfError::BadRelaTable);
        }
        let count = dyn_.rela_size / dyn_.rela_ent;
        for i in 0..count {
            let off = (dyn_.rela_offset + i * dyn_.rela_ent) as usize;
            let r_offset = u64::from_le_bytes(img[off..off + 8].try_into().unwrap());
            let r_info = u64::from_le_bytes(img[off + 8..off + 16].try_into().unwrap());
            let r_addend = u64::from_le_bytes(img[off + 16..off + 24].try_into().unwrap());
            let r_type = r_info & 0xFFFF_FFFF;
            let target = base
                .checked_add(r_offset)
                .ok_or(ElfError::RelocOutOfBounds)?;
            let target_end = target.checked_add(8).ok_or(ElfError::RelocOutOfBounds)?;
            // 边界校验：写入目标（虚拟地址）必须完全落在某个可写段内
            //（段边界算术 checked——恶意 base/vaddr 回绕不得绕过检查）。
            let in_writable = segments.iter().any(|s| {
                s.flags.write
                    && target >= base.saturating_add(s.vaddr)
                    && target_end
                        <= base
                            .checked_add(s.vaddr)
                            .and_then(|b| b.checked_add(s.memsz))
                            .unwrap_or(0)
            });
            if !in_writable {
                return Err(ElfError::RelocOutOfBounds);
            }
            match r_type {
                R_AARCH64_RELATIVE => {
                    let value = base.wrapping_add(r_addend);
                    // 写入槽 = r_offset（链接地址 == 文件偏移的平坦布局）；
                    // 审查修复：越 img 界 → Err（旧实现 `img[slot..slot+8]`
                    // 直接 panic——装配层传入未铺到段末端的镜像即触发）。
                    let slot = r_offset as usize;
                    let slot_end = slot.checked_add(8).ok_or(ElfError::RelocOutOfBounds)?;
                    if slot_end > img.len() {
                        return Err(ElfError::RelocOutOfBounds);
                    }
                    img[slot..slot_end].copy_from_slice(&value.to_le_bytes());
                }
                _ => return Err(ElfError::UnsupportedReloc),
            }
        }
        Ok(())
    }

    /// Total virtual memory size (sum of all segment memsz).
    pub fn total_vmem(&self) -> u64 {
        self.segments.iter().map(|s| s.memsz).sum()
    }

    /// WP-37.4：统一装载深接口（纯布局 + staging 内重定位；页映射由装配
    /// 层按 [`LoadedImage`] 执行——loader 保持纯函数，不依赖 seL4）。
    ///
    /// `image` = 完整 ELF 字节；`policy` = 装载策略（ET_EXEC 允许性 +
    /// ET_DYN bias）；`staging` = 调用方提供的可写缓冲（装载后虚拟地址
    /// 空间的平坦镜像，长度必须 ≥ 装载跨度——[`ElfError::StagingTooSmall`]）。
    ///
    /// 处理（WP-37.4 #1–#12 纯函数侧）：
    /// 1. ET_EXEC 固定地址冲突 → `PolicyRejectsExec`（策略拒绝时）；
    /// 2. ET_DYN bias 页对齐校验（`BiasMisaligned`）；
    /// 3. 段 `p_vaddr != p_offset` 真实 ELF：文件数据按 `file_offset`
    ///    拷入 staging 的装载地址槽位（与 apply_relocations 的平坦
    ///    布局假设解耦）；
    /// 4. filesz/memsz、BSS 清零（staging 槽位零填充）；
    /// 5. dynamic 表 / RELA 边界（parse_dynamic + 表界校验）；
    /// 6. relocation 白名单（[`is_allowed_relocation`]）；
    /// 7. overflow/越界：装载地址 checked_add、staging 槽位界内；
    /// 8. relocation 写入目标必须落在本 image **可写** 段（`RelocOutOfBounds`）；
    /// 9. 段权限原样保留（text=R-X 等；W^X 由 parse 保证）；
    /// 10. I/D cache：reloc 后若有 exec 段 → `icache_after_reloc = true`
    ///     （装配层负责 cache 维护）；
    /// 11. entrypoint 必须落在 exec 段（parse 已保证）并完成 bias 平移；
    /// 12. 中途失败：无副作用（staging 为调用方缓冲，错误路径不产生
    ///     需回收的映射——映射层错误回收由装配层负责）。
    pub fn load_verified_elf(
        image: &[u8],
        policy: &ElfLoadPolicy,
        staging: &mut [u8],
    ) -> Result<LoadedImage, ElfError> {
        let elf = Self::parse(image)?;
        let e_type = u16::from_le_bytes(image[16..18].try_into().unwrap());
        let is_dyn = e_type == ET_DYN;
        if !is_dyn && !policy.allow_et_exec {
            return Err(ElfError::PolicyRejectsExec);
        }
        if is_dyn && policy.dyn_bias % 0x1000 != 0 {
            return Err(ElfError::BiasMisaligned);
        }

        // 装载后段区间（bias 平移；ET_EXEC 保持链接地址）。
        let mut loaded: alloc::vec::Vec<(u64, u64, &LoadedSegment)> = alloc::vec::Vec::new();
        let mut min_loaded = u64::MAX;
        let mut max_loaded_end = 0u64;
        for seg in elf.segments() {
            let start = if is_dyn {
                policy
                    .dyn_bias
                    .checked_add(seg.vaddr)
                    .ok_or(ElfError::SegmentOverlap)?
            } else {
                seg.vaddr
            };
            let end = start
                .checked_add(seg.memsz)
                .ok_or(ElfError::SegmentOverlap)?;
            min_loaded = min_loaded.min(start);
            max_loaded_end = max_loaded_end.max(end);
            loaded.push((start, end, seg));
        }
        let span = max_loaded_end.saturating_sub(min_loaded);
        if (span as usize) > staging.len() {
            return Err(ElfError::StagingTooSmall);
        }

        // 段数据拷入 staging（槽位 = 装载后 vaddr - min_loaded）；BSS 由
        // 缓冲零初始化覆盖（staging 以零初始化后按 filesz 拷贝）。
        staging.fill(0);
        for (start, _, seg) in &loaded {
            let slot = (*start - min_loaded) as usize;
            let file_start = seg.offset as usize;
            let file_end = file_start
                .checked_add(seg.filesz as usize)
                .ok_or(ElfError::SegmentOutOfBounds)?;
            if file_end > image.len() {
                return Err(ElfError::SegmentOutOfBounds);
            }
            staging[slot..slot + seg.filesz as usize].copy_from_slice(&image[file_start..file_end]);
        }

        // ET_DYN 重定位（真实 ELF：DT_RELA 为文件偏移语义；写入 = bias +
        // r_offset 虚拟地址 → staging 槽位）。
        let mut needs_relocation = false;
        let mut icache_after_reloc = false;
        if is_dyn && Self::has_pt_dynamic(image)? {
            let dyn_info = Self::parse_dynamic(image)?;
            // rela 表边界（文件偏移）。
            let rela_end = dyn_info
                .rela_offset
                .checked_add(dyn_info.rela_size)
                .ok_or(ElfError::BadRelaTable)?;
            if rela_end > image.len() as u64 {
                return Err(ElfError::BadRelaTable);
            }
            if dyn_info.rela_ent < ELF64_RELA_SIZE
                || dyn_info.rela_ent % 8 != 0
                || dyn_info.rela_size % dyn_info.rela_ent != 0
            {
                return Err(ElfError::BadRelaTable);
            }
            let count = dyn_info.rela_size / dyn_info.rela_ent;
            for i in 0..count {
                let off = (dyn_info.rela_offset + i * dyn_info.rela_ent) as usize;
                let r_offset = u64::from_le_bytes(image[off..off + 8].try_into().unwrap());
                let r_info = u64::from_le_bytes(image[off + 8..off + 16].try_into().unwrap());
                let r_addend = u64::from_le_bytes(image[off + 16..off + 24].try_into().unwrap());
                let r_type = r_info & 0xFFFF_FFFF;
                if !is_allowed_relocation(r_type) {
                    return Err(ElfError::UnsupportedReloc);
                }
                let target = policy
                    .dyn_bias
                    .checked_add(r_offset)
                    .ok_or(ElfError::RelocOutOfBounds)?;
                let target_end = target.checked_add(8).ok_or(ElfError::RelocOutOfBounds)?;
                // 写入目标必须在某可写段内（W^X 下 exec 段不可写）。
                let in_writable = loaded.iter().any(|(start, end, seg)| {
                    seg.flags.write && target >= *start && target_end <= *end
                });
                if !in_writable {
                    return Err(ElfError::RelocOutOfBounds);
                }
                let slot = (target - min_loaded) as usize;
                let slot_end = slot.checked_add(8).ok_or(ElfError::RelocOutOfBounds)?;
                if slot_end > staging.len() {
                    return Err(ElfError::RelocOutOfBounds);
                }
                let value = policy.dyn_bias.wrapping_add(r_addend);
                staging[slot..slot_end].copy_from_slice(&value.to_le_bytes());
                needs_relocation = true;
            }
            // I/D cache：reloc 后存在 exec 段（装配层映射前须维护 cache）。
            icache_after_reloc = needs_relocation && elf.has_executable();
        }

        // 装载结果：段计划（升序 vaddr）+ 入口（bias 平移）。
        let mut segments: alloc::vec::Vec<LoadedImageSegment> = loaded
            .iter()
            .map(|(start, _, seg)| LoadedImageSegment {
                vaddr: *start,
                memsz: seg.memsz,
                filesz: seg.filesz,
                file_offset: seg.offset,
                permissions: seg.flags,
            })
            .collect();
        segments.sort_unstable_by_key(|s| s.vaddr);
        let entry = if is_dyn {
            policy
                .dyn_bias
                .checked_add(elf.entry())
                .ok_or(ElfError::EntryNotInExecutableSegment)?
        } else {
            elf.entry()
        };
        Ok(LoadedImage {
            entry,
            segments,
            needs_relocation,
            icache_after_reloc,
        })
    }

    /// True if any segment is executable.
    pub fn has_executable(&self) -> bool {
        self.segments.iter().any(|s| s.flags.execute)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    /// Build a minimal valid ELF64 PIE AArch64 binary in memory.
    fn build_minimal_elf() -> Vec<u8> {
        // ELF header (64 bytes) + 1 PT_LOAD program header (56 bytes) + 8 bytes of code.
        let mut buf = vec![0u8; ELF64_EHDR_SIZE + ELF64_PHDR_SIZE + 8];

        // e_ident
        buf[0..4].copy_from_slice(&ELFMAG);
        buf[4] = ELFCLASS64;
        buf[5] = ELFDATA2LSB;
        buf[6] = 1; // EV_CURRENT

        // e_type = ET_DYN
        buf[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
        // e_machine = EM_AARCH64
        buf[18..20].copy_from_slice(&EM_AARCH64.to_le_bytes());

        // e_entry = 0x1000 (within the PT_LOAD segment)
        let entry: u64 = 0x1000;
        buf[24..32].copy_from_slice(&entry.to_le_bytes());

        // e_phoff = 64 (right after ELF header)
        let phoff: u64 = ELF64_EHDR_SIZE as u64;
        buf[32..40].copy_from_slice(&phoff.to_le_bytes());

        // e_phentsize = 56
        buf[54..56].copy_from_slice(&(ELF64_PHDR_SIZE as u16).to_le_bytes());
        // e_phnum = 1
        buf[56..58].copy_from_slice(&1u16.to_le_bytes());

        // Program header at offset 64.
        let ph = ELF64_EHDR_SIZE;
        // p_type = PT_LOAD
        buf[ph..ph + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
        // p_flags = PF_R | PF_X (read + execute, not writable → W^X OK)
        buf[ph + 4..ph + 8].copy_from_slice(&(PF_R | PF_X).to_le_bytes());
        // p_offset = ELF64_EHDR_SIZE + ELF64_PHDR_SIZE (code starts after headers)
        let code_offset: u64 = (ELF64_EHDR_SIZE + ELF64_PHDR_SIZE) as u64;
        buf[ph + 8..ph + 16].copy_from_slice(&code_offset.to_le_bytes());
        // p_vaddr = 0x1000 (matches entry)
        let vaddr: u64 = 0x1000;
        buf[ph + 16..ph + 24].copy_from_slice(&vaddr.to_le_bytes());
        // p_filesz = 8
        buf[ph + 32..ph + 40].copy_from_slice(&8u64.to_le_bytes());
        // p_memsz = 8
        buf[ph + 40..ph + 48].copy_from_slice(&8u64.to_le_bytes());
        // p_align = 0x1000
        buf[ph + 48..ph + 56].copy_from_slice(&0x1000u64.to_le_bytes());

        // Code: 8 bytes (e.g., NOP + RET).
        buf[ELF64_EHDR_SIZE + ELF64_PHDR_SIZE] = 0x1f; // NOP
        buf[ELF64_EHDR_SIZE + ELF64_PHDR_SIZE + 1] = 0x20;
        buf[ELF64_EHDR_SIZE + ELF64_PHDR_SIZE + 2] = 0x03;
        buf[ELF64_EHDR_SIZE + ELF64_PHDR_SIZE + 3] = 0xd5;
        buf[ELF64_EHDR_SIZE + ELF64_PHDR_SIZE + 4] = 0xc0; // RET
        buf[ELF64_EHDR_SIZE + ELF64_PHDR_SIZE + 5] = 0x03;
        buf[ELF64_EHDR_SIZE + ELF64_PHDR_SIZE + 6] = 0x5f;
        buf[ELF64_EHDR_SIZE + ELF64_PHDR_SIZE + 7] = 0xd6;

        buf
    }

    #[test]
    fn test_parse_valid_elf() {
        let elf_data = build_minimal_elf();
        let elf = LoadedElf::parse(&elf_data).expect("valid ELF should parse");
        assert_eq!(elf.entry(), 0x1000);
        assert_eq!(elf.segments().len(), 1);
        assert!(elf.has_executable());
        assert_eq!(elf.total_vmem(), 8);
    }

    #[test]
    fn test_reject_truncated_header() {
        let short = [0u8; 10];
        assert_eq!(
            LoadedElf::parse(&short).unwrap_err(),
            ElfError::TruncatedHeader
        );
    }

    #[test]
    fn test_reject_bad_magic() {
        let mut elf_data = build_minimal_elf();
        elf_data[0] = 0x00; // corrupt magic
        assert_eq!(LoadedElf::parse(&elf_data).unwrap_err(), ElfError::BadMagic);
    }

    #[test]
    fn test_reject_not_elf64() {
        let mut elf_data = build_minimal_elf();
        elf_data[4] = 1; // ELFCLASS32
        assert_eq!(LoadedElf::parse(&elf_data).unwrap_err(), ElfError::NotElf64);
    }

    #[test]
    fn test_accept_et_exec() {
        // ET_EXEC（固定地址裸机 EL0/rootserver，C2/BOOT-007）必须被接受。
        let mut elf_data = build_minimal_elf();
        elf_data[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        assert!(LoadedElf::parse(&elf_data).is_ok());
    }

    #[test]
    fn test_reject_et_rel() {
        // ET_REL（可重定位目标文件，无固定段）仍必须拒绝。
        let mut elf_data = build_minimal_elf();
        elf_data[16..18].copy_from_slice(&1u16.to_le_bytes()); // ET_REL
        assert_eq!(LoadedElf::parse(&elf_data).unwrap_err(), ElfError::NotPie);
    }

    #[test]
    fn test_reject_wrong_machine() {
        let mut elf_data = build_minimal_elf();
        elf_data[18..20].copy_from_slice(&62u16.to_le_bytes()); // EM_X86_64
        assert_eq!(
            LoadedElf::parse(&elf_data).unwrap_err(),
            ElfError::WrongMachine
        );
    }

    #[test]
    fn test_reject_wx_violation() {
        let mut elf_data = build_minimal_elf();
        let ph = ELF64_EHDR_SIZE;
        // Set flags to PF_R | PF_W | PF_X (W+X violation).
        elf_data[ph + 4..ph + 8].copy_from_slice(&(PF_R | PF_W | PF_X).to_le_bytes());
        assert_eq!(
            LoadedElf::parse(&elf_data).unwrap_err(),
            ElfError::WxViolation
        );
    }

    #[test]
    fn test_reject_no_load_segments() {
        let mut elf_data = build_minimal_elf();
        let ph = ELF64_EHDR_SIZE;
        // Change PT_LOAD to PT_NULL (0).
        elf_data[ph..ph + 4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(
            LoadedElf::parse(&elf_data).unwrap_err(),
            ElfError::NoLoadSegments
        );
    }

    #[test]
    fn test_reject_entry_not_in_executable() {
        let mut elf_data = build_minimal_elf();
        // Set entry to 0x9999 (not within the segment at 0x1000).
        elf_data[24..32].copy_from_slice(&0x9999u64.to_le_bytes());
        assert_eq!(
            LoadedElf::parse(&elf_data).unwrap_err(),
            ElfError::EntryNotInExecutableSegment
        );
    }

    #[test]
    fn test_multiple_segments() {
        // Build an ELF with 2 PT_LOAD segments: one R-X (text), one RW- (data).
        let header_size = ELF64_EHDR_SIZE;
        let phdr_size = ELF64_PHDR_SIZE * 2;
        let text_size = 8u64;
        let data_size = 16u64;
        let total = header_size + phdr_size + text_size as usize + data_size as usize;

        let mut buf = vec![0u8; total];

        // ELF header.
        buf[0..4].copy_from_slice(&ELFMAG);
        buf[4] = ELFCLASS64;
        buf[5] = ELFDATA2LSB;
        buf[6] = 1;
        buf[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
        buf[18..20].copy_from_slice(&EM_AARCH64.to_le_bytes());
        buf[24..32].copy_from_slice(&0x1000u64.to_le_bytes()); // entry in text
        buf[32..40].copy_from_slice(&(header_size as u64).to_le_bytes()); // phoff
        buf[54..56].copy_from_slice(&(ELF64_PHDR_SIZE as u16).to_le_bytes());
        buf[56..58].copy_from_slice(&2u16.to_le_bytes()); // 2 program headers

        // PT_LOAD #0: R-X (text) at vaddr 0x1000.
        let ph0 = header_size;
        buf[ph0..ph0 + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
        buf[ph0 + 4..ph0 + 8].copy_from_slice(&(PF_R | PF_X).to_le_bytes());
        let text_off = (header_size + phdr_size) as u64;
        buf[ph0 + 8..ph0 + 16].copy_from_slice(&text_off.to_le_bytes());
        buf[ph0 + 16..ph0 + 24].copy_from_slice(&0x1000u64.to_le_bytes());
        buf[ph0 + 32..ph0 + 40].copy_from_slice(&text_size.to_le_bytes());
        buf[ph0 + 40..ph0 + 48].copy_from_slice(&text_size.to_le_bytes());
        buf[ph0 + 48..ph0 + 56].copy_from_slice(&0x1000u64.to_le_bytes());

        // PT_LOAD #1: RW- (data) at vaddr 0x2000.
        let ph1 = header_size + ELF64_PHDR_SIZE;
        buf[ph1..ph1 + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
        buf[ph1 + 4..ph1 + 8].copy_from_slice(&(PF_R | PF_W).to_le_bytes());
        let data_off = (header_size + phdr_size + text_size as usize) as u64;
        buf[ph1 + 8..ph1 + 16].copy_from_slice(&data_off.to_le_bytes());
        buf[ph1 + 16..ph1 + 24].copy_from_slice(&0x2000u64.to_le_bytes());
        buf[ph1 + 32..ph1 + 40].copy_from_slice(&data_size.to_le_bytes());
        buf[ph1 + 40..ph1 + 48].copy_from_slice(&data_size.to_le_bytes());
        buf[ph1 + 48..ph1 + 56].copy_from_slice(&0x1000u64.to_le_bytes());

        let elf = LoadedElf::parse(&buf).expect("should parse");
        assert_eq!(elf.segments().len(), 2);
        assert!(elf.segments()[0].flags.execute);
        assert!(!elf.segments()[0].flags.write);
        assert!(elf.segments()[1].flags.write);
        assert!(!elf.segments()[1].flags.execute);
        assert_eq!(elf.total_vmem(), text_size + data_size);
    }

    /// 30 RT-005 判据：段边界回绕（vaddr+memsz 溢出）→ validate_segments
    /// 必须 Err（修复前普通加法回绕为小数，入口校验被绕过）。
    #[test]
    fn test_load_verified_elf_two_biases_relocate() {
        // WP-37.4 #1/#2：构造真实形态 ET_DYN（vaddr≠offset + PT_DYNAMIC +
        // R_AARCH64_RELATIVE），同一 ELF 两个不同 bias 装载：
        // - 布局整体平移（entry/段 vaddr）；
        // - staging 内 reloc 写入值 = bias + addend（不同 bias 值不同）。
        let image = build_pie_with_rela();
        let mut staging = vec![0u8; 0x3000];
        let policy_a = ElfLoadPolicy {
            allow_et_exec: false,
            dyn_bias: 0x400_000,
        };
        let img_a = LoadedElf::load_verified_elf(&image, &policy_a, &mut staging).expect("load A");
        assert_eq!(img_a.entry, 0x401_000, "entry 必须随 bias 平移");
        assert_eq!(img_a.segments.len(), 2);
        assert_eq!(img_a.segments[0].vaddr, 0x401_000);
        assert_eq!(img_a.segments[1].vaddr, 0x402_000);
        assert!(img_a.needs_relocation);
        assert!(
            img_a.icache_after_reloc,
            "reloc 后存在 exec 段 → 需 cache 维护"
        );
        // reloc 写入槽：target(0x400000+0x2000=0x402000) - min(0x401000) = 0x1000。
        let slot = 0x1000usize;
        let expected = 0x400_000u64.wrapping_add(0x1234);
        assert_eq!(
            u64::from_le_bytes(staging[slot..slot + 8].try_into().unwrap()),
            expected,
            "reloc 值必须 = bias + addend"
        );
        // 文本内容也在 staging（file_offset≠vaddr 的真实 ELF 形态）。
        assert_eq!(staging[0], 0x1f, "text 首字节应拷入装载槽位");

        // 不同 bias → reloc 值不同、entry 不同（同一 ELF 两实例）。
        let policy_b = ElfLoadPolicy {
            allow_et_exec: false,
            dyn_bias: 0x800_000,
        };
        let mut staging_b = vec![0u8; 0x3000];
        let img_b =
            LoadedElf::load_verified_elf(&image, &policy_b, &mut staging_b).expect("load B");
        assert_eq!(img_b.entry, 0x801_000);
        assert_eq!(
            u64::from_le_bytes(staging_b[slot..slot + 8].try_into().unwrap()),
            0x800_000u64.wrapping_add(0x1234)
        );
    }

    /// WP-37.4 负例：#6 未知 reloc 类型拒绝；#8 写入只读段拒绝；
    /// staging 不足拒绝；bias 非页对齐拒绝；ET_EXEC 策略拒绝。
    #[test]
    fn test_load_verified_elf_negative_cases() {
        let image = build_pie_with_rela();
        // 未知 reloc 类型（r_info 低 32 位 = 999）。
        let mut bad_type = image.clone();
        bad_type[0x2A0 + 8..0x2A0 + 16].copy_from_slice(&999u64.to_le_bytes());
        let mut staging = vec![0u8; 0x3000];
        assert_eq!(
            LoadedElf::load_verified_elf(
                &bad_type,
                &ElfLoadPolicy {
                    allow_et_exec: false,
                    dyn_bias: 0x400_000
                },
                &mut staging
            ),
            Err(ElfError::UnsupportedReloc)
        );
        // 写入只读段（r_offset=0x1000 落在 text R-X 段）→ RelocOutOfBounds。
        let mut bad_target = image.clone();
        bad_target[0x2A0..0x2A0 + 8].copy_from_slice(&0x1000u64.to_le_bytes());
        assert_eq!(
            LoadedElf::load_verified_elf(
                &bad_target,
                &ElfLoadPolicy {
                    allow_et_exec: false,
                    dyn_bias: 0x400_000
                },
                &mut staging
            ),
            Err(ElfError::RelocOutOfBounds)
        );
        // staging 不足 → StagingTooSmall。
        let mut small = vec![0u8; 0x100];
        assert_eq!(
            LoadedElf::load_verified_elf(
                &image,
                &ElfLoadPolicy {
                    allow_et_exec: false,
                    dyn_bias: 0x400_000
                },
                &mut small
            ),
            Err(ElfError::StagingTooSmall)
        );
        // bias 非页对齐 → BiasMisaligned。
        let mut ok_staging = vec![0u8; 0x3000];
        assert_eq!(
            LoadedElf::load_verified_elf(
                &image,
                &ElfLoadPolicy {
                    allow_et_exec: false,
                    dyn_bias: 0x400_001
                },
                &mut ok_staging
            ),
            Err(ElfError::BiasMisaligned)
        );
        // ET_EXEC + policy 拒绝 → PolicyRejectsExec；允许时布局保持链接地址。
        let mut exec_elf = build_minimal_elf();
        exec_elf[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
        assert_eq!(
            LoadedElf::load_verified_elf(
                &exec_elf,
                &ElfLoadPolicy {
                    allow_et_exec: false,
                    dyn_bias: 0x400_000
                },
                &mut ok_staging
            ),
            Err(ElfError::PolicyRejectsExec)
        );
        let exec_img = LoadedElf::load_verified_elf(
            &exec_elf,
            &ElfLoadPolicy {
                allow_et_exec: true,
                dyn_bias: 0x400_000,
            },
            &mut ok_staging,
        )
        .expect("ET_EXEC 允许");
        assert_eq!(exec_img.entry, 0x1000, "ET_EXEC 保持链接地址");
        assert!(!exec_img.needs_relocation);
    }

    /// 构造真实形态 ET_DYN：text(offset=0x200,vaddr=0x1000,RX) +
    /// data(offset=0x240,vaddr=0x2000,RW) + PT_DYNAMIC(offset=0x260) +
    /// RELA 表(offset=0x2A0)。rela：r_offset=0x2000、RELATIVE、addend=0x1234。
    fn build_pie_with_rela() -> Vec<u8> {
        let header = ELF64_EHDR_SIZE;
        // phdr 表从 e_phoff（=ELF header 末尾 64）开始，非 header+56。
        let ph = header;
        let rela_at = 0x2A0usize;
        let mut buf = vec![0u8; rela_at + ELF64_RELA_SIZE as usize];
        buf[..4].copy_from_slice(&ELFMAG);
        buf[4] = ELFCLASS64;
        buf[5] = ELFDATA2LSB;
        buf[6] = 1;
        buf[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
        buf[18..20].copy_from_slice(&EM_AARCH64.to_le_bytes());
        buf[24..32].copy_from_slice(&0x1000u64.to_le_bytes()); // e_entry
        buf[32..40].copy_from_slice(&(header as u64).to_le_bytes()); // e_phoff
        buf[54..56].copy_from_slice(&(ELF64_PHDR_SIZE as u16).to_le_bytes());
        buf[56..58].copy_from_slice(&3u16.to_le_bytes()); // e_phnum

        // ph[0]: text R+X / ph[1]: data R+W / ph[2]: PT_DYNAMIC。
        let mut phdr =
            |idx: usize, p_type: u32, off: u64, vaddr: u64, filesz: u64, memsz: u64, flags: u32| {
                let o = ph + idx * ELF64_PHDR_SIZE;
                buf[o..o + 4].copy_from_slice(&p_type.to_le_bytes());
                buf[o + 4..o + 8].copy_from_slice(&flags.to_le_bytes());
                buf[o + 8..o + 16].copy_from_slice(&off.to_le_bytes());
                buf[o + 16..o + 24].copy_from_slice(&vaddr.to_le_bytes());
                buf[o + 24..o + 32].copy_from_slice(&vaddr.to_le_bytes());
                buf[o + 32..o + 40].copy_from_slice(&filesz.to_le_bytes());
                buf[o + 40..o + 48].copy_from_slice(&memsz.to_le_bytes());
                buf[o + 48..o + 56].copy_from_slice(&0x1000u64.to_le_bytes());
            };
        phdr(0, PT_LOAD, 0x200, 0x1000, 0x40, 0x40, PF_R | PF_X);
        phdr(1, PT_LOAD, 0x240, 0x2000, 0x20, 0x20, PF_R | PF_W);
        phdr(2, PT_DYNAMIC, 0x260, 0x3000, 0x58, 0x58, PF_R | PF_W);
        // text 内容。
        buf[0x200..0x200 + 8].copy_from_slice(&[0x1f, 0x20, 0x03, 0xd5, 0xc0, 0x03, 0x5f, 0xd6]);
        // dynamic 表（0x260，4 条目 ×16B = 64B → 0x2A0）：
        // DT_RELA=0x2A0 / DT_RELASZ=24 / DT_RELAENT=24 / DT_NULL=0。
        buf[0x260..0x268].copy_from_slice(&DT_RELA.to_le_bytes());
        buf[0x268..0x270].copy_from_slice(&(rela_at as u64).to_le_bytes());
        buf[0x270..0x278].copy_from_slice(&DT_RELASZ.to_le_bytes());
        buf[0x278..0x280].copy_from_slice(&ELF64_RELA_SIZE.to_le_bytes());
        buf[0x280..0x288].copy_from_slice(&DT_RELAENT.to_le_bytes());
        buf[0x288..0x290].copy_from_slice(&ELF64_RELA_SIZE.to_le_bytes());
        buf[0x290..0x298].copy_from_slice(&DT_NULL.to_le_bytes());
        // rela 表 @0x2A0：r_offset=0x2000（data 段链接地址）、RELATIVE、addend=0x1234。
        buf[rela_at..rela_at + 8].copy_from_slice(&0x2000u64.to_le_bytes());
        buf[rela_at + 8..rela_at + 16].copy_from_slice(&R_AARCH64_RELATIVE.to_le_bytes());
        buf[rela_at + 16..rela_at + 24].copy_from_slice(&0x1234u64.to_le_bytes());
        buf
    }
    #[test]
    fn test_validate_segments_rejects_wrapping_range() {
        let segs = alloc::vec![LoadedSegment {
            vaddr: 0xFFFF_FFFF_FFFF_F000,
            memsz: 0x2000,
            filesz: 0,
            offset: 0,
            flags: SegmentFlags {
                read: true,
                write: false,
                execute: true,
            },
            align: 0x1000,
        }];
        assert_eq!(
            LoadedElf::validate_segments(&segs),
            Err(ElfError::SegmentOverlap)
        );
    }

    /// 30 RT-006 判据：两段 vaddr 区间重叠 → SegmentOverlap。
    #[test]
    fn test_validate_segments_rejects_overlap() {
        let seg = |vaddr: u64, memsz: u64| LoadedSegment {
            vaddr,
            memsz,
            filesz: 0,
            offset: 0,
            flags: SegmentFlags {
                read: true,
                write: false,
                execute: true,
            },
            align: 0x1000,
        };
        // [0x1000, 0x3000) 与 [0x2000, 0x4000) 重叠。
        let segs = alloc::vec![seg(0x1000, 0x2000), seg(0x2000, 0x2000)];
        assert_eq!(
            LoadedElf::validate_segments(&segs),
            Err(ElfError::SegmentOverlap)
        );
        // 相邻不重叠：[0x1000, 0x2000) 与 [0x2000, 0x3000) 合法。
        let ok = alloc::vec![seg(0x1000, 0x1000), seg(0x2000, 0x1000)];
        assert!(LoadedElf::validate_segments(&ok).is_ok());
    }

    /// 构造最小重定位测试镜像：文件偏移 0x100 处为 .rela.dyn（3 条
    /// R_AARCH64_RELATIVE），PT_DYNAMIC 指向它。
    fn relocation_fixture() -> (
        alloc::vec::Vec<u8>,
        DynamicInfo,
        alloc::vec::Vec<LoadedSegment>,
    ) {
        // 布局：段（可写，vaddr=0x1000，memsz=0x2000，含 3 个 8B 槽）+ rela 表。
        let mut img = alloc::vec![0u8; 0x4000];
        // 段：vaddr 0x1000，文件偏移 0x1000，memsz 0x2000（可写）。
        for slot in [0x1000usize, 0x1010, 0x1020] {
            img[slot..slot + 8].copy_from_slice(&0xDEAD_BEEFu64.to_le_bytes());
        }
        // .rela.dyn @ 0x3000：3 条 RELATIVE（r_offset 相对链接地址）。
        let rela_off = 0x3000u64;
        let relas: [(u64, u64, u64); 3] = [
            (0x1000, R_AARCH64_RELATIVE, 0x1111),
            (0x1010, R_AARCH64_RELATIVE, 0x2222),
            (0x1020, R_AARCH64_RELATIVE, 0x3333),
        ];
        for (i, (off, typ, addend)) in relas.iter().enumerate() {
            let at = (rela_off + i as u64 * 24) as usize;
            img[at..at + 8].copy_from_slice(&off.to_le_bytes());
            img[at + 8..at + 16].copy_from_slice(&typ.to_le_bytes());
            img[at + 16..at + 24].copy_from_slice(&addend.to_le_bytes());
        }
        let dyn_info = DynamicInfo {
            rela_offset: rela_off,
            rela_size: 3 * 24,
            rela_ent: 24,
        };
        let segments = alloc::vec![LoadedSegment {
            vaddr: 0x1000,
            memsz: 0x2000,
            filesz: 0x2000,
            offset: 0x1000,
            flags: SegmentFlags {
                read: true,
                write: true,
                execute: false,
            },
            align: 0x1000,
        }];
        (img, dyn_info, segments)
    }

    /// 30 WP-27.7 判据正例（黄金向量）：3 条 RELATIVE 在非零 base 下全部
    /// 解析为 `base + r_addend`（静态数据/函数指针/字符串三类语义）。
    #[test]
    fn apply_relocations_golden_vector() {
        let (mut img, dyn_info, segments) = relocation_fixture();
        let base = 0x4000_0000u64;
        LoadedElf::apply_relocations(&mut img, base, &dyn_info, &segments).expect("relocate");
        let read_u64 = |off: usize| u64::from_le_bytes(img[off..off + 8].try_into().unwrap());
        assert_eq!(read_u64(0x1000), base + 0x1111);
        assert_eq!(read_u64(0x1010), base + 0x2222);
        assert_eq!(read_u64(0x1020), base + 0x3333);
    }

    /// 30 WP-27.7 判据负例：恶意 Rela 写入目标越出可写段（指向段外）→
    /// RelocOutOfBounds（防任意写）。
    #[test]
    fn apply_relocations_rejects_out_of_bounds() {
        let (mut img, mut dyn_info, segments) = relocation_fixture();
        dyn_info.rela_offset = 0x3000;
        // 把第一条 r_offset 改成段外（0x5000 > 段末 0x1000+0x2000）。
        img[0x3000..0x3008].copy_from_slice(&0x5000u64.to_le_bytes());
        assert_eq!(
            LoadedElf::apply_relocations(&mut img, 0x4000_0000, &dyn_info, &segments),
            Err(ElfError::RelocOutOfBounds)
        );
        // 指向只读段同样拒绝（segments 只有可写段，0x0 不在任何段）。
        img[0x3000..0x3008].copy_from_slice(&0x100u64.to_le_bytes());
        assert_eq!(
            LoadedElf::apply_relocations(&mut img, 0x4000_0000, &dyn_info, &segments),
            Err(ElfError::RelocOutOfBounds)
        );
    }

    /// 30 WP-27.7 判据负例：不支持的 Rela 类型 → UnsupportedReloc
    /// （fail-closed，不静默跳过）。
    #[test]
    fn apply_relocations_rejects_unsupported_type() {
        let (mut img, dyn_info, segments) = relocation_fixture();
        // 第一条 r_info 类型改为 0x401（R_AARCH64_GLOB_DAT——不支持）。
        let typ = 0x401u64;
        img[0x3008..0x3010].copy_from_slice(&typ.to_le_bytes());
        assert_eq!(
            LoadedElf::apply_relocations(&mut img, 0x4000_0000, &dyn_info, &segments),
            Err(ElfError::UnsupportedReloc)
        );
    }

    /// 审查修复判据：写入槽超出 img 界（装配层传入未铺到段末端的镜像，
    /// 如文件字节）→ RelocOutOfBounds（旧实现 `img[slot..slot+8]` 切片
    /// panic——库公开契约不得 panic）。
    #[test]
    fn apply_relocations_rejects_slot_beyond_img() {
        let (mut img, dyn_info, mut segments) = relocation_fixture();
        // 段链接地址抬到 rela 表之后（vaddr=0x5000，base=0）：in_writable
        // 检查通过（target=0x5000 ∈ [0x5000, 0x7000)），但写入槽 0x5000
        // 超出 img（rela 表 @ 0x3000 之后无内容）——必须 Err 而非 panic。
        segments[0].vaddr = 0x5000;
        img[0x3000..0x3008].copy_from_slice(&0x5000u64.to_le_bytes());
        assert_eq!(
            LoadedElf::apply_relocations(&mut img, 0, &dyn_info, &segments),
            Err(ElfError::RelocOutOfBounds)
        );
    }

    /// 审查修复判据：rela_size 非 rela_ent 整数倍 → BadRelaTable
    /// （旧实现余数条目被静默忽略）。
    #[test]
    fn apply_relocations_rejects_non_integral_rela_size() {
        let (mut img, mut dyn_info, segments) = relocation_fixture();
        dyn_info.rela_size = 3 * 24 + 8; // 余 8B
        assert_eq!(
            LoadedElf::apply_relocations(&mut img, 0x4000_0000, &dyn_info, &segments),
            Err(ElfError::BadRelaTable)
        );
    }

    /// 30 WP-27.7 判据负例：PT_DYNAMIC 缺失 → BadDynamic。
    #[test]
    fn parse_dynamic_rejects_missing() {
        // 无 PT_DYNAMIC 的最小 ELF 头（phnum=0）。
        let mut data = alloc::vec![0u8; ELF64_EHDR_SIZE];
        data[0..4].copy_from_slice(&ELFMAG);
        data[4] = ELFCLASS64;
        data[5] = ELFDATA2LSB;
        data[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
        data[18..20].copy_from_slice(&EM_AARCH64.to_le_bytes());
        data[54..56].copy_from_slice(&(ELF64_PHDR_SIZE as u16).to_le_bytes());
        assert_eq!(LoadedElf::parse_dynamic(&data), Err(ElfError::BadDynamic));
    }
}
