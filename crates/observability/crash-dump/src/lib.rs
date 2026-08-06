//! EnerOS 崩溃转储格式与 build-id 精确匹配 (OBS-007).
//!
//! MASTER §21.13 OBS-007: "crash dump symbolize — build id 精确匹配"。
//!
//! 本 crate 提供：
//! - [`BuildId`] — 从 ELF `.note.gnu.build-id` 提取的构建标识符（≤32 字节）。
//! - [`CrashDump`] — 崩溃转储结构（build-id + 寄存器 + 帧 + panic 消息）。
//! - [`encode`] / [`decode`] — 紧凑二进制编解码（含 CRC32 校验）。
//! - [`extract_build_id`] — 从原始 ELF 字节解析 `.note.gnu.build-id`。
//! - [`verify_build_id_match`] — 精确匹配校验（fail-closed：不匹配则拒绝）。
//!
//! # 设计
//!
//! 崩溃转储在设备端捕获（panic handler 中），包含崩溃二进制的 build-id、
//! 寄存器快照（PC/SP/LR）和回溯帧地址。符号化在主机端完成（xtask
//! `symbolize` 命令），主机端必须验证转储中的 build-id 与待符号化的 ELF
//! build-id 完全一致，否则拒绝符号化（fail-closed）。
//!
//! # no_std 合规
//!
//! `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。非测试代码仅使用
//! `core::*` / `alloc::*` / `heapless::*`，不调用 `panic!` / `todo!` /
//! `unimplemented!` / `unwrap()`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod build_id;
pub mod format;

pub use build_id::{extract_build_id, BuildId, BUILD_ID_MAX_LEN};
pub use format::{
    crc32, decode, encode, Arch, CrashDump, CrashDumpError, CrashFrame, MAGIC, VERSION,
};

/// 验证崩溃转储的 build-id 与 ELF 的 build-id 是否精确匹配。
///
/// OBS-007 核心要求：build id 精确匹配。两个 build-id 必须完全一致（字节级），
/// 否则返回 `false`（fail-closed：拒绝符号化）。
///
/// 空 build-id（长度为 0）视为不可信，返回 `false`。
pub fn verify_build_id_match(dump: &CrashDump, elf_bytes: &[u8]) -> bool {
    if dump.build_id.is_empty() {
        return false;
    }
    match extract_build_id(elf_bytes) {
        Some(elf_id) => dump.build_id.as_bytes() == elf_id.as_bytes(),
        None => false,
    }
}

/// 符号化结果（主机端使用，no_std 兼容以便测试）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFrame {
    /// 原始地址。
    pub address: u64,
    /// 符号名（如 `eneros_kernel::sched::dispatch`）。
    pub symbol: Option<alloc::string::String>,
    /// 符号起始地址。
    pub symbol_start: Option<u64>,
    /// 帧内偏移（address - symbol_start）。
    pub offset: Option<u64>,
    /// 源文件路径（如有 DWARF 调试信息）。
    pub file: Option<alloc::string::String>,
    /// 源行号。
    pub line: Option<u32>,
    /// 模块名（crate 或 binary 名）。
    pub module: Option<alloc::string::String>,
}
