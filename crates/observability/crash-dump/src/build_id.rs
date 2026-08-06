//! 从原始 ELF 字节提取 `.note.gnu.build-id`（OBS-007）。
//!
//! 解析 ELF64 头 → 节头表 → `.shstrtab` → 找到 `.note.gnu.build-id` 节 →
//! 解析 note 条目 → 提取 `NT_GNU_BUILD_ID` (type=3) 描述符。
//!
//! 同时检查 PT_NOTE 程序头作为后备路径（某些链接器只放程序头）。
//!
//! # no_std 合规
//!
//! 仅使用 `core::*` / `alloc::*`，无系统调用。

use alloc::string::String;

/// build-id 最大长度（SHA-256 = 32 字节；GNU build-id 通常为 20 字节 SHA-1）。
pub const BUILD_ID_MAX_LEN: usize = 32;

/// NT_GNU_BUILD_ID 的 note type 值。
const NT_GNU_BUILD_ID: u32 = 3;

/// ELF 魔数。
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// ELFCLASS64。
const ELFCLASS64: u8 = 2;

/// SHT_NOTE 节类型。
const SHT_NOTE: u32 = 7;

/// PT_NOTE 程序头类型。
const PT_NOTE: u32 = 4;

/// 构建标识符（≤32 字节）。
#[derive(Clone, PartialEq, Eq)]
pub struct BuildId {
    bytes: heapless::Vec<u8, BUILD_ID_MAX_LEN>,
}

impl BuildId {
    /// 从字节切片构造；超过 `BUILD_ID_MAX_LEN` 返回 `None`。
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > BUILD_ID_MAX_LEN {
            return None;
        }
        let mut buf = heapless::Vec::new();
        for &b in bytes {
            if buf.push(b).is_err() {
                return None;
            }
        }
        Some(Self { bytes: buf })
    }

    /// 返回 build-id 字节。
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// 返回 build-id 的十六进制字符串（小写）。
    pub fn to_hex(&self) -> String {
        use core::fmt::Write;
        let mut s = String::with_capacity(self.bytes.len() * 2);
        for &b in &self.bytes {
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// 长度。
    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

impl core::fmt::Debug for BuildId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "BuildId(")?;
        for &b in &self.bytes {
            write!(f, "{b:02x}")?;
        }
        write!(f, ")")
    }
}

/// 从原始 ELF 字节提取 `.note.gnu.build-id`。
///
/// 返回 `None` 的情形：
/// - 不是有效的 ELF64 文件
/// - 没有 `.note.gnu.build-id` 节且没有 `PT_NOTE` 程序头
/// - build-id 超过 `BUILD_ID_MAX_LEN`
/// - note 格式损坏
pub fn extract_build_id(elf: &[u8]) -> Option<BuildId> {
    if elf.len() < 64 || elf[..4] != ELF_MAGIC || elf[4] != ELFCLASS64 {
        return None;
    }
    // 优先从 .note.gnu.build-id 节提取
    if let Some(id) = extract_from_section(elf) {
        return Some(id);
    }
    // 后备：从 PT_NOTE 程序头提取
    extract_from_program_header(elf)
}

/// 小端读 u16。
fn rd_u16(buf: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*buf.get(off)?, *buf.get(off + 1)?]))
}

/// 小端读 u32。
fn rd_u32(buf: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *buf.get(off)?,
        *buf.get(off + 1)?,
        *buf.get(off + 2)?,
        *buf.get(off + 3)?,
    ]))
}

/// 小端读 u64。
fn rd_u64(buf: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes([
        *buf.get(off)?,
        *buf.get(off + 1)?,
        *buf.get(off + 2)?,
        *buf.get(off + 3)?,
        *buf.get(off + 4)?,
        *buf.get(off + 5)?,
        *buf.get(off + 6)?,
        *buf.get(off + 7)?,
    ]))
}

/// ELF64 头偏移量。
struct Ehdr64 {
    e_shoff: u64,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
    e_phoff: u64,
    e_phentsize: u16,
    e_phnum: u16,
}

fn parse_ehdr64(elf: &[u8]) -> Option<Ehdr64> {
    if elf.len() < 64 {
        return None;
    }
    // 验证 ELF 数据编码（ELFDATA2LSB = 1，小端）
    if elf[5] != 1 {
        return None;
    }
    Some(Ehdr64 {
        e_shoff: rd_u64(elf, 0x28)?,
        e_shentsize: rd_u16(elf, 0x3a)?,
        e_shnum: rd_u16(elf, 0x3c)?,
        e_shstrndx: rd_u16(elf, 0x3e)?,
        e_phoff: rd_u64(elf, 0x20)?,
        e_phentsize: rd_u16(elf, 0x36)?,
        e_phnum: rd_u16(elf, 0x38)?,
    })
}

/// ELF64 节头。
struct Shdr64 {
    sh_name: u32,
    sh_type: u32,
    sh_offset: u64,
    sh_size: u64,
}

fn parse_shdr64(elf: &[u8], ehdr: &Ehdr64, idx: u16) -> Option<Shdr64> {
    let base = ehdr.e_shoff as usize + idx as usize * ehdr.e_shentsize as usize;
    if base + 64 > elf.len() {
        return None;
    }
    Some(Shdr64 {
        sh_name: rd_u32(elf, base)?,
        sh_type: rd_u32(elf, base + 4)?,
        sh_offset: rd_u64(elf, base + 24)?,
        sh_size: rd_u64(elf, base + 32)?,
    })
}

/// ELF64 程序头。
struct Phdr64 {
    p_type: u32,
    p_offset: u64,
    p_filesz: u64,
}

fn parse_phdr64(elf: &[u8], ehdr: &Ehdr64, idx: u16) -> Option<Phdr64> {
    let base = ehdr.e_phoff as usize + idx as usize * ehdr.e_phentsize as usize;
    if base + 56 > elf.len() {
        return None;
    }
    Some(Phdr64 {
        p_type: rd_u32(elf, base)?,
        p_offset: rd_u64(elf, base + 8)?,
        p_filesz: rd_u64(elf, base + 32)?,
    })
}

/// 从 `.note.gnu.build-id` 节提取 build-id。
fn extract_from_section(elf: &[u8]) -> Option<BuildId> {
    let ehdr = parse_ehdr64(elf)?;
    if ehdr.e_shnum == 0 || ehdr.e_shentsize < 64 {
        return None;
    }
    // 读取 shstrtab
    let strtab_hdr = parse_shdr64(elf, &ehdr, ehdr.e_shstrndx)?;
    let strtab_start = strtab_hdr.sh_offset as usize;
    let strtab_end = strtab_start.checked_add(strtab_hdr.sh_size as usize)?;

    // 遍历节头，找到 .note.gnu.build-id
    for idx in 0..ehdr.e_shnum {
        let shdr = parse_shdr64(elf, &ehdr, idx)?;
        // 节名：从 shstrtab 偏移 sh_name 处读取 C 字符串
        let name_off = strtab_start.checked_add(shdr.sh_name as usize)?;
        let name = read_cstr(elf, name_off, strtab_end)?;
        if name == b".note.gnu.build-id" || (shdr.sh_type == SHT_NOTE && name == b".note") {
            let sec_start = shdr.sh_offset as usize;
            let sec_end = sec_start.checked_add(shdr.sh_size as usize)?;
            let sec = elf.get(sec_start..sec_end)?;
            if let Some(id) = parse_notes(sec, true) {
                return Some(id);
            }
        }
    }
    None
}

/// 从 PT_NOTE 程序头提取 build-id（后备路径）。
fn extract_from_program_header(elf: &[u8]) -> Option<BuildId> {
    let ehdr = parse_ehdr64(elf)?;
    if ehdr.e_phnum == 0 || ehdr.e_phentsize < 56 {
        return None;
    }
    for idx in 0..ehdr.e_phnum {
        let phdr = parse_phdr64(elf, &ehdr, idx)?;
        if phdr.p_type != PT_NOTE {
            continue;
        }
        let seg_start = phdr.p_offset as usize;
        let seg_end = seg_start.checked_add(phdr.p_filesz as usize)?;
        let seg = elf.get(seg_start..seg_end)?;
        if let Some(id) = parse_notes(seg, false) {
            return Some(id);
        }
    }
    None
}

/// 解析 note 条目，查找 `NT_GNU_BUILD_ID`。
///
/// `require_gnu_name`：为 true 时要求 note name 为 "GNU"（节路径）；
/// 为 false 时（程序头路径）接受任意 name（更宽松，因为 PT_NOTE 可能
/// 合并多个 note）。
fn parse_notes(data: &[u8], require_gnu_name: bool) -> Option<BuildId> {
    let mut off = 0usize;
    while off + 12 <= data.len() {
        let namesz = rd_u32(data, off)? as usize;
        let descsz = rd_u32(data, off + 4)? as usize;
        let ntype = rd_u32(data, off + 8)?;
        let name_start = off + 12;
        // name 对齐 4 字节
        let name_padded = (namesz + 3) & !3;
        let name_end = name_start.checked_add(name_padded)?;
        let desc_start = name_end;
        let desc_padded = (descsz + 3) & !3;
        let desc_end = desc_start.checked_add(desc_padded)?;
        if desc_end > data.len() {
            return None;
        }
        if ntype == NT_GNU_BUILD_ID {
            // 验证 name（"GNU\0" 或 "GNU"）
            let name = data.get(name_start..name_start + namesz)?;
            let is_gnu = name.starts_with(b"GNU");
            if !require_gnu_name || is_gnu {
                let desc = data.get(desc_start..desc_start + descsz)?;
                return BuildId::from_bytes(desc);
            }
        }
        off = desc_end;
    }
    None
}

/// 读取 C 字符串（以 null 结尾）。
fn read_cstr(buf: &[u8], start: usize, limit: usize) -> Option<&[u8]> {
    let mut end = start;
    while end < limit {
        if *buf.get(end)? == 0 {
            return buf.get(start..end);
        }
        end += 1;
    }
    None
}
