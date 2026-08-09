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

/// Parsed and validated ELF64 binary.
#[derive(Debug)]
pub struct LoadedElf {
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
        let entry_in_exec = segments
            .iter()
            .any(|s| s.flags.execute && e_entry >= s.vaddr && e_entry < s.vaddr + s.memsz);
        if !entry_in_exec {
            return Err(ElfError::EntryNotInExecutableSegment);
        }

        Ok(Self {
            entry: e_entry,
            segments,
        })
    }

    /// Entry point virtual address.
    pub fn entry(&self) -> u64 {
        self.entry
    }

    /// All PT_LOAD segments.
    pub fn segments(&self) -> &[LoadedSegment] {
        &self.segments
    }

    /// Total virtual memory size (sum of all segment memsz).
    pub fn total_vmem(&self) -> u64 {
        self.segments.iter().map(|s| s.memsz).sum()
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
}
