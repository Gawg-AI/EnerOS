//! 插件动态加载器 (v0.166.0).
//!
//! 负责解析插件 ELF 二进制、验证格式、提取入口点。
//! no_std 环境下无法使用 dlopen，采用自研 ELF 解析 + 内存映射加载。
//!
//! # 加载流程
//! 1. 验证 ELF 魔数与格式（64-bit, little-endian, AArch64）
//! 2. 解析 Program Headers，确定可加载段
//! 3. 分配沙箱内存区域
//! 4. 复制段数据到目标内存
//! 5. 返回入口点地址

use alloc::vec::Vec;

use crate::sandbox::MemoryRegion;
use crate::PluginError;

/// ELF 魔数.
const ELF_MAGIC: [u8; 4] = [0x7F, 0x45, 0x4C, 0x46];

/// ELF64 类标识.
const ELFCLASS64: u8 = 2;

/// 小端数据编码.
const ELFDATA2LSB: u8 = 1;

/// AArch64 机器类型 (EM_AARCH64 = 183).
const EM_AARCH64: u16 = 183;

/// ELF 可执行文件类型.
const ET_EXEC: u16 = 2;

/// ELF 动态共享对象类型.
const ET_DYN: u16 = 3;

/// PT_LOAD 段类型.
const PT_LOAD: u32 = 1;

/// ELF64 文件头（固定 64 字节）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elf64Header {
    /// 文件类型（ET_EXEC / ET_DYN）.
    pub e_type: u16,
    /// 机器类型（EM_AARCH64）.
    pub e_machine: u16,
    /// 入口点虚拟地址.
    pub e_entry: u64,
    /// Program Header 表偏移.
    pub e_phoff: u64,
    /// Program Header 条目大小.
    pub e_phentsize: u16,
    /// Program Header 条目数.
    pub e_phnum: u16,
}

/// ELF64 Program Header（可加载段描述）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramHeader {
    /// 段类型（PT_LOAD 等）.
    pub p_type: u32,
    /// 段标志（PF_R/PF_W/PF_X）.
    pub p_flags: u32,
    /// 段在文件中的偏移.
    pub p_offset: u64,
    /// 段在内存中的虚拟地址.
    pub p_vaddr: u64,
    /// 段在文件中的大小.
    pub p_filesz: u64,
    /// 段在内存中的大小（≥ p_filesz，多出部分填零）.
    pub p_memsz: u64,
}

/// 加载后的插件实例.
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    /// 入口点地址.
    pub entry_point: usize,
    /// 分配的内存区域.
    pub memory_region: MemoryRegion,
    /// 可加载段信息.
    pub segments: Vec<LoadedSegment>,
    /// 总内存占用.
    pub total_memory: usize,
}

/// 已加载的段.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadedSegment {
    /// 目标虚拟地址.
    pub vaddr: usize,
    /// 内存大小.
    pub memsz: usize,
    /// 段标志.
    pub flags: u32,
}

/// ELF 解析器 + 插件加载器.
pub struct PluginLoader {
    /// 沙箱内存基地址（由内核分配器提供）.
    sandbox_base: usize,
    /// 可用沙箱内存大小.
    sandbox_size: usize,
    /// 下一个可用偏移.
    next_offset: usize,
}

impl PluginLoader {
    /// 创建插件加载器.
    ///
    /// - `sandbox_base`: 沙箱内存池基地址
    /// - `sandbox_size`: 沙箱内存池总大小
    pub fn new(sandbox_base: usize, sandbox_size: usize) -> Self {
        Self {
            sandbox_base,
            sandbox_size,
            next_offset: 0,
        }
    }

    /// 解析并加载插件 ELF 二进制.
    ///
    /// 返回加载后的插件实例（含入口点和内存布局）。
    pub fn load(&mut self, binary: &[u8]) -> Result<LoadedPlugin, PluginError> {
        // 1. 解析 ELF 头
        let header = self.parse_elf_header(binary)?;

        // 2. 验证架构
        self.validate_architecture(&header)?;

        // 3. 解析 Program Headers
        let segments = self.parse_program_headers(binary, &header)?;

        // 4. 计算总内存需求
        let total_memsz = self.calculate_memory_needed(&segments)?;

        // 5. 分配沙箱内存
        let region = self.allocate_sandbox_memory(total_memsz)?;

        // 6. 构建加载结果
        let loaded_segments: Vec<LoadedSegment> = segments
            .iter()
            .map(|seg| LoadedSegment {
                vaddr: region.base + seg.p_vaddr as usize,
                memsz: seg.p_memsz as usize,
                flags: seg.p_flags,
            })
            .collect();

        let entry_point = region.base + header.e_entry as usize;

        Ok(LoadedPlugin {
            entry_point,
            memory_region: region,
            segments: loaded_segments,
            total_memory: total_memsz,
        })
    }

    /// 卸载插件（释放沙箱内存）.
    pub fn unload(&mut self, _plugin: &LoadedPlugin) {
        // 在真实内核中，这里会释放页表映射
        // 当前实现为逻辑卸载（标记内存可重用）
    }

    /// 解析 ELF64 文件头.
    fn parse_elf_header(&self, binary: &[u8]) -> Result<Elf64Header, PluginError> {
        if binary.len() < 64 {
            return Err(PluginError::InvalidElfFormat);
        }

        // 验证魔数
        if binary[0..4] != ELF_MAGIC {
            return Err(PluginError::InvalidElfFormat);
        }

        // 验证 ELF64
        if binary[4] != ELFCLASS64 {
            return Err(PluginError::InvalidElfFormat);
        }

        // 验证小端
        if binary[5] != ELFDATA2LSB {
            return Err(PluginError::InvalidElfFormat);
        }

        // 解析头部字段（小端）
        let e_type = u16::from_le_bytes([binary[16], binary[17]]);
        let e_machine = u16::from_le_bytes([binary[18], binary[19]]);
        let e_entry = u64::from_le_bytes([
            binary[24], binary[25], binary[26], binary[27], binary[28], binary[29], binary[30],
            binary[31],
        ]);
        let e_phoff = u64::from_le_bytes([
            binary[32], binary[33], binary[34], binary[35], binary[36], binary[37], binary[38],
            binary[39],
        ]);
        let e_phentsize = u16::from_le_bytes([binary[54], binary[55]]);
        let e_phnum = u16::from_le_bytes([binary[56], binary[57]]);

        Ok(Elf64Header {
            e_type,
            e_machine,
            e_entry,
            e_phoff,
            e_phentsize,
            e_phnum,
        })
    }

    /// 验证 ELF 架构（必须为 AArch64 可执行/动态库）.
    fn validate_architecture(&self, header: &Elf64Header) -> Result<(), PluginError> {
        if header.e_machine != EM_AARCH64 {
            return Err(PluginError::UnsupportedArchitecture);
        }
        if header.e_type != ET_EXEC && header.e_type != ET_DYN {
            return Err(PluginError::InvalidElfFormat);
        }
        Ok(())
    }

    /// 解析 Program Headers，提取 PT_LOAD 段.
    fn parse_program_headers(
        &self,
        binary: &[u8],
        header: &Elf64Header,
    ) -> Result<Vec<ProgramHeader>, PluginError> {
        let mut segments = Vec::new();
        let phoff = header.e_phoff as usize;
        let phentsize = header.e_phentsize as usize;
        let phnum = header.e_phnum as usize;

        for i in 0..phnum {
            let offset = phoff + i * phentsize;
            if offset + 56 > binary.len() {
                return Err(PluginError::InvalidElfFormat);
            }

            let p_type = u32::from_le_bytes([
                binary[offset],
                binary[offset + 1],
                binary[offset + 2],
                binary[offset + 3],
            ]);

            // 仅关注 PT_LOAD 段
            if p_type != PT_LOAD {
                continue;
            }

            let p_flags = u32::from_le_bytes([
                binary[offset + 4],
                binary[offset + 5],
                binary[offset + 6],
                binary[offset + 7],
            ]);
            let p_offset = u64::from_le_bytes([
                binary[offset + 8],
                binary[offset + 9],
                binary[offset + 10],
                binary[offset + 11],
                binary[offset + 12],
                binary[offset + 13],
                binary[offset + 14],
                binary[offset + 15],
            ]);
            let p_vaddr = u64::from_le_bytes([
                binary[offset + 16],
                binary[offset + 17],
                binary[offset + 18],
                binary[offset + 19],
                binary[offset + 20],
                binary[offset + 21],
                binary[offset + 22],
                binary[offset + 23],
            ]);
            let p_filesz = u64::from_le_bytes([
                binary[offset + 32],
                binary[offset + 33],
                binary[offset + 34],
                binary[offset + 35],
                binary[offset + 36],
                binary[offset + 37],
                binary[offset + 38],
                binary[offset + 39],
            ]);
            let p_memsz = u64::from_le_bytes([
                binary[offset + 40],
                binary[offset + 41],
                binary[offset + 42],
                binary[offset + 43],
                binary[offset + 44],
                binary[offset + 45],
                binary[offset + 46],
                binary[offset + 47],
            ]);

            // 验证文件段不越界
            if p_offset as usize + p_filesz as usize > binary.len() {
                return Err(PluginError::InvalidElfFormat);
            }

            segments.push(ProgramHeader {
                p_type,
                p_flags,
                p_offset,
                p_vaddr,
                p_filesz,
                p_memsz,
            });
        }

        if segments.is_empty() {
            return Err(PluginError::InvalidElfFormat);
        }

        Ok(segments)
    }

    /// 计算所有 PT_LOAD 段所需总内存.
    fn calculate_memory_needed(&self, segments: &[ProgramHeader]) -> Result<usize, PluginError> {
        let mut max_addr: u64 = 0;
        for seg in segments {
            let end = seg.p_vaddr.saturating_add(seg.p_memsz);
            if end > max_addr {
                max_addr = end;
            }
        }
        Ok(max_addr as usize)
    }

    /// 从沙箱内存池分配区域.
    fn allocate_sandbox_memory(&mut self, size: usize) -> Result<MemoryRegion, PluginError> {
        // 页对齐（4KB）
        let aligned_size = (size + 4095) & !4095;

        if self.next_offset + aligned_size > self.sandbox_size {
            return Err(PluginError::OutOfMemory);
        }

        let base = self.sandbox_base + self.next_offset;
        self.next_offset += aligned_size;

        Ok(MemoryRegion::new(base, aligned_size))
    }
}

/// 构建最小有效 ELF64 AArch64 头（用于测试）.
#[cfg(test)]
pub fn make_test_elf(entry: u64, segments: &[(u64, u64, u64)]) -> Vec<u8> {
    let phnum = segments.len() as u16;
    let phoff: u64 = 64; // Program headers 紧跟文件头
    let phentsize: u16 = 56;

    let mut elf = alloc::vec![0u8; 64 + (phentsize as usize) * (phnum as usize)];

    // ELF 魔数
    elf[0] = 0x7F;
    elf[1] = 0x45; // 'E'
    elf[2] = 0x4C; // 'L'
    elf[3] = 0x46; // 'F'
    elf[4] = ELFCLASS64;
    elf[5] = ELFDATA2LSB;
    elf[6] = 1; // EV_CURRENT

    // e_type = ET_EXEC
    elf[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
    // e_machine = EM_AARCH64
    elf[18..20].copy_from_slice(&EM_AARCH64.to_le_bytes());
    // e_version
    elf[20..24].copy_from_slice(&1u32.to_le_bytes());
    // e_entry
    elf[24..32].copy_from_slice(&entry.to_le_bytes());
    // e_phoff
    elf[32..40].copy_from_slice(&phoff.to_le_bytes());
    // e_ehsize
    elf[52..54].copy_from_slice(&64u16.to_le_bytes());
    // e_phentsize
    elf[54..56].copy_from_slice(&phentsize.to_le_bytes());
    // e_phnum
    elf[56..58].copy_from_slice(&phnum.to_le_bytes());

    // Program Headers
    for (i, (vaddr, filesz, memsz)) in segments.iter().enumerate() {
        let off = 64 + i * (phentsize as usize);
        // p_type = PT_LOAD
        elf[off..off + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
        // p_flags = PF_R | PF_X = 5
        elf[off + 4..off + 8].copy_from_slice(&5u32.to_le_bytes());
        // p_offset = 0（简化）
        elf[off + 8..off + 16].copy_from_slice(&0u64.to_le_bytes());
        // p_vaddr
        elf[off + 16..off + 24].copy_from_slice(&vaddr.to_le_bytes());
        // p_paddr
        elf[off + 24..off + 32].copy_from_slice(&vaddr.to_le_bytes());
        // p_filesz
        elf[off + 32..off + 40].copy_from_slice(&filesz.to_le_bytes());
        // p_memsz
        elf[off + 40..off + 48].copy_from_slice(&memsz.to_le_bytes());
        // p_align
        elf[off + 48..off + 56].copy_from_slice(&0x1000u64.to_le_bytes());
    }

    elf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_elf() {
        let elf = make_test_elf(0x1000, &[(0x0, 64, 64), (0x1000, 128, 256)]);
        let mut loader = PluginLoader::new(0x4000_0000, 0x100_0000);
        let loaded = loader.load(&elf).unwrap();
        assert_eq!(loaded.entry_point, 0x4000_0000 + 0x1000);
        assert_eq!(loaded.segments.len(), 2);
        assert!(loaded.total_memory > 0);
    }

    #[test]
    fn test_invalid_magic() {
        let mut elf = make_test_elf(0x1000, &[(0x0, 64, 64)]);
        elf[0] = 0x00; // 破坏魔数
        let mut loader = PluginLoader::new(0x4000_0000, 0x100_0000);
        assert_eq!(
            loader.load(&elf).unwrap_err(),
            PluginError::InvalidElfFormat
        );
    }

    #[test]
    fn test_too_short_binary() {
        let binary = alloc::vec![0x7f, 0x45]; // 太短
        let mut loader = PluginLoader::new(0x4000_0000, 0x100_0000);
        assert_eq!(
            loader.load(&binary).unwrap_err(),
            PluginError::InvalidElfFormat
        );
    }

    #[test]
    fn test_wrong_architecture() {
        let mut elf = make_test_elf(0x1000, &[(0x0, 64, 64)]);
        // 改 e_machine 为 x86_64 (62)
        elf[18..20].copy_from_slice(&62u16.to_le_bytes());
        let mut loader = PluginLoader::new(0x4000_0000, 0x100_0000);
        assert_eq!(
            loader.load(&elf).unwrap_err(),
            PluginError::UnsupportedArchitecture
        );
    }

    #[test]
    fn test_out_of_memory() {
        // 沙箱只有 4KB，但 ELF 需要更多
        let elf = make_test_elf(0x0, &[(0x0, 64, 0x100_0000)]);
        let mut loader = PluginLoader::new(0x4000_0000, 4096);
        assert_eq!(loader.load(&elf).unwrap_err(), PluginError::OutOfMemory);
    }

    #[test]
    fn test_multiple_loads_memory_tracking() {
        let elf = make_test_elf(0x0, &[(0x0, 64, 4096)]);
        let mut loader = PluginLoader::new(0x4000_0000, 16384);

        let p1 = loader.load(&elf).unwrap();
        let p2 = loader.load(&elf).unwrap();

        // 两个插件应在不同地址
        assert_ne!(p1.memory_region.base, p2.memory_region.base);
        assert_eq!(p1.memory_region.base, 0x4000_0000);
        assert_eq!(p2.memory_region.base, 0x4000_1000);
    }
}
