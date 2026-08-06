//! OBS-007 主机端崩溃转储符号化器。
//!
//! 读取设备端捕获的崩溃转储（`eneros-crash-dump` 格式），验证其 build-id
//! 与待符号化的 ELF 文件的 build-id 精确匹配，然后解析 ELF 的符号表
//! （`.symtab` + `.strtab`）将回溯帧地址解析为 `{symbol, offset, file}`。
//!
//! # fail-closed
//!
//! - build-id 不匹配 → 拒绝符号化，退出码 2。
//! - ELF 无法解析 → 拒绝，退出码 2。
//! - 转储 CRC 损坏 → 拒绝，退出码 2。
//! - 单个地址无法解析 → 输出 `unresolved`，不拒绝（正常：地址可能在
//!   linker stub 或 stripped 段）。

use std::fs;
use std::path::Path;

use eneros_crash_dump::{verify_build_id_match, CrashDump};
use object::{Object, ObjectSection, ObjectSymbol};
use serde::Serialize;

use crate::DynResult;

pub fn command_symbolize(_root: &Path, args: &[String]) -> DynResult<()> {
    let (dump_path, elf_path) = match args {
        [d, e] => (d.as_str(), e.as_str()),
        _ => {
            return Err(invalid("symbolize requires <crash-dump.bin> <elf-file>"));
        }
    };
    let dump_bytes = fs::read(dump_path)?;
    let elf_bytes = fs::read(elf_path)?;
    let dump = eneros_crash_dump::decode(&dump_bytes)
        .map_err(|e| invalid(format!("crash dump decode failed: {e:?}")))?;

    // OBS-007 核心：build-id 精确匹配
    if !verify_build_id_match(&dump, &elf_bytes) {
        return Err(invalid(format!(
            "build-id mismatch: dump build-id {:?} does not match ELF build-id — refusing to symbolize (OBS-007 fail-closed)",
            dump.build_id
        )));
    }

    let object =
        object::File::parse(&*elf_bytes).map_err(|e| invalid(format!("ELF parse failed: {e}")))?;

    let report = symbolize_dump(&dump, &object)?;
    let json = serde_json::to_string_pretty(&report)?;
    println!("{json}");
    Ok(())
}

#[derive(Debug, Serialize)]
struct SymbolizeReport {
    schema: &'static str,
    result: &'static str,
    build_id_hex: String,
    crash_ts_ns: u64,
    thread_id: u64,
    pc: u64,
    sp: u64,
    lr: u64,
    arch: String,
    panic_msg: String,
    frame_count: usize,
    frames: Vec<FrameReport>,
}

#[derive(Debug, Serialize)]
struct FrameReport {
    index: usize,
    address: u64,
    symbol: Option<String>,
    symbol_start: Option<u64>,
    offset: Option<u64>,
    section: Option<String>,
    resolved: bool,
}

fn symbolize_dump(dump: &CrashDump, object: &object::File) -> DynResult<SymbolizeReport> {
    // 构建符号地址索引（按地址排序，用于二分查找最近的符号）
    let mut symbols: Vec<(u64, u64, String)> = Vec::new(); // (start, end, name)
    for symbol in object.symbols() {
        let name = match symbol.name() {
            Ok(n) => n.to_owned(),
            _ => continue,
        };
        let addr = symbol.address();
        let size = symbol.size();
        if size == 0 {
            continue;
        }
        symbols.push((addr, addr + size, name));
    }
    // 动态符号也加入（stripped binary 可能只有 .dynsym）
    for symbol in object.dynamic_symbols() {
        let name = match symbol.name() {
            Ok(n) => n.to_owned(),
            _ => continue,
        };
        let addr = symbol.address();
        let size = symbol.size();
        if size == 0 {
            continue;
        }
        symbols.push((addr, addr + size, name));
    }
    symbols.sort_by_key(|(start, _, _)| *start);

    let section_names: Vec<(u64, u64, String)> = object
        .sections()
        .filter_map(|s| {
            let name = s.name().ok()?.to_owned();
            Some((s.address(), s.address() + s.size(), name))
        })
        .collect::<Vec<_>>();

    let mut frame_reports = Vec::with_capacity(dump.frames.len());
    for (i, frame) in dump.frames.iter().enumerate() {
        let resolved = resolve_address(frame.pc, &symbols, &section_names);
        let is_resolved = resolved.symbol.is_some();
        frame_reports.push(FrameReport {
            index: i,
            address: frame.pc,
            symbol: resolved.symbol,
            symbol_start: resolved.symbol_start,
            offset: resolved.offset,
            section: resolved.section,
            resolved: is_resolved,
        });
    }

    // 同时解析 PC/LR
    let _pc_resolved = resolve_address(dump.pc, &symbols, &section_names);
    let _lr_resolved = resolve_address(dump.lr, &symbols, &section_names);

    Ok(SymbolizeReport {
        schema: "eneros.xtask.symbolize.v1",
        result: "PASS",
        build_id_hex: dump.build_id.to_hex(),
        crash_ts_ns: dump.crash_ts_ns,
        thread_id: dump.thread_id,
        pc: dump.pc,
        sp: dump.sp,
        lr: dump.lr,
        arch: format!("{:?}", dump.arch).to_ascii_lowercase(),
        panic_msg: dump.panic_msg.clone(),
        frame_count: dump.frames.len(),
        frames: frame_reports,
    })
}

struct Resolved {
    symbol: Option<String>,
    symbol_start: Option<u64>,
    offset: Option<u64>,
    section: Option<String>,
}

fn resolve_address(
    addr: u64,
    symbols: &[(u64, u64, String)],
    sections: &[(u64, u64, String)],
) -> Resolved {
    // 二分查找：找到 start <= addr < end 的符号
    let symbol = symbols
        .binary_search_by(|(start, end, _)| {
            if addr < *start {
                core::cmp::Ordering::Greater
            } else if addr >= *end {
                core::cmp::Ordering::Less
            } else {
                core::cmp::Ordering::Equal
            }
        })
        .ok()
        .map(|idx| &symbols[idx]);

    let section = sections
        .iter()
        .find(|(start, end, _)| addr >= *start && addr < *end)
        .map(|(_, _, name)| name.clone());

    Resolved {
        symbol: symbol.map(|(_, _, name)| name.clone()),
        symbol_start: symbol.map(|(start, _, _)| *start),
        offset: symbol.map(|(start, _, _)| addr - start),
        section,
    }
}

fn invalid(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_address_finds_containing_symbol() {
        let symbols: Vec<(u64, u64, String)> = vec![
            (0x40000000, 0x40000100, "func_a".to_owned()),
            (0x40000100, 0x40000180, "func_b".to_owned()),
        ];
        let sections: Vec<(u64, u64, String)> = vec![(0x40000000, 0x40001000, ".text".to_owned())];

        let r = resolve_address(0x40000050, &symbols, &sections);
        assert_eq!(r.symbol.as_deref(), Some("func_a"));
        assert_eq!(r.symbol_start, Some(0x40000000));
        assert_eq!(r.offset, Some(0x50));
        assert_eq!(r.section.as_deref(), Some(".text"));

        let r = resolve_address(0x40000120, &symbols, &sections);
        assert_eq!(r.symbol.as_deref(), Some("func_b"));
        assert_eq!(r.offset, Some(0x20));
    }

    #[test]
    fn resolve_address_at_symbol_start() {
        let symbols: Vec<(u64, u64, String)> = vec![(0x40000000, 0x40000100, "func_a".to_owned())];
        let sections: Vec<(u64, u64, String)> = vec![];

        // 地址恰好在符号起始处
        let r = resolve_address(0x40000000, &symbols, &sections);
        assert_eq!(r.symbol.as_deref(), Some("func_a"));
        assert_eq!(r.offset, Some(0));
    }

    #[test]
    fn resolve_address_at_symbol_end_excluded() {
        let symbols: Vec<(u64, u64, String)> = vec![(0x40000000, 0x40000100, "func_a".to_owned())];
        let sections: Vec<(u64, u64, String)> = vec![];

        // 地址在符号结束处（不包含）
        let r = resolve_address(0x40000100, &symbols, &sections);
        assert!(r.symbol.is_none());
    }

    #[test]
    fn resolve_address_returns_none_for_unknown() {
        let symbols: Vec<(u64, u64, String)> = vec![(0x40000000, 0x40000100, "func_a".to_owned())];
        let sections: Vec<(u64, u64, String)> = vec![];

        let r = resolve_address(0x50000000, &symbols, &sections);
        assert!(r.symbol.is_none());
        assert!(r.section.is_none());
    }

    #[test]
    fn resolve_address_finds_section_without_symbol() {
        let symbols: Vec<(u64, u64, String)> = vec![];
        let sections: Vec<(u64, u64, String)> = vec![(0x40000000, 0x40001000, ".text".to_owned())];

        // 地址在 section 内但无符号
        let r = resolve_address(0x40000050, &symbols, &sections);
        assert!(r.symbol.is_none());
        assert_eq!(r.section.as_deref(), Some(".text"));
    }

    #[test]
    fn binary_search_handles_unsorted_gap() {
        // 符号之间有间隙
        let symbols: Vec<(u64, u64, String)> = vec![
            (0x40000000, 0x40000080, "func_a".to_owned()),
            (0x40000100, 0x40000200, "func_b".to_owned()),
        ];
        let sections: Vec<(u64, u64, String)> = vec![];

        // 地址在间隙中
        let r = resolve_address(0x40000090, &symbols, &sections);
        assert!(r.symbol.is_none());
    }
}
