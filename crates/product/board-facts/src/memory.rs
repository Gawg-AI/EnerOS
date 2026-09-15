//! 内存区事实的成对提取与重叠判定（WP-70.12.1「错 RAM 重叠负例使准入红」）。
//!
//! 约定：RAM 事实按索引成对登记——
//! `ram/base[0]`（单位 `addr`）+ `ram/size[0]`（单位 `MiB`/`GiB`/…），
//! 索引不要求连续，但**任一半存在且 `verified=true` 时另一半必须存在**，
//! 否则该窗口不可核对（拒绝，不静默跳过）。

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::capability::Subsystem;
use crate::fact::Fact;

/// 地址未知的显式标记（未冻结窗口用它占位，**不构成事实**）。
pub const MEM_ADDR_UNKNOWN: &str = "unknown";

/// 一个内存窗口（已验证事实推导）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySpan {
    pub index: u32,
    pub base: u64,
    pub size: u64,
}

impl MemorySpan {
    /// 末字节地址（不含）。
    pub fn end(&self) -> u64 {
        self.base.saturating_add(self.size)
    }

    /// 与另一窗口是否重叠（零长度窗口不重叠）。
    pub fn overlaps(&self, other: &MemorySpan) -> bool {
        self.size != 0 && other.size != 0 && self.base < other.end() && other.base < self.end()
    }
}

/// 窗口提取失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryError {
    /// 成对缺失（存在一半且已验证）。
    UnpairedIndex { index: u32, missing: &'static str },
    /// 地址/容量不可解析。
    Unparsable {
        index: u32,
        field: &'static str,
        value: String,
    },
    /// 容量为 0。
    ZeroSize { index: u32 },
    /// 两个窗口重叠。
    Overlap { a: u32, b: u32 },
}

impl MemoryError {
    pub fn reason(&self) -> String {
        match self {
            MemoryError::UnpairedIndex { index, missing } => {
                format!("ram-unpaired-index:{index}:missing={missing}")
            }
            MemoryError::Unparsable {
                index,
                field,
                value,
            } => format!("ram-unparsable:{index}:{field}={value}"),
            MemoryError::ZeroSize { index } => format!("ram-zero-size:{index}"),
            MemoryError::Overlap { a, b } => format!("ram-overlap:{a}:{b}"),
        }
    }
}

/// 从事实集合提取并校验内存窗口（只消费 `verified=true` 的事实——
/// 未验证窗口不构成平台事实，故不参与重叠判定）。
pub fn spans_from_facts(facts: &[Fact]) -> Result<Vec<MemorySpan>, MemoryError> {
    let mut bases: Vec<(u32, u64)> = Vec::new();
    let mut sizes: Vec<(u32, u64)> = Vec::new();
    for f in facts {
        if f.subsystem != Subsystem::Ram || !f.verified {
            continue;
        }
        match parse_mem_key(&f.key) {
            Some((MemField::Base, idx)) => {
                let v = parse_addr(&f.value).ok_or_else(|| MemoryError::Unparsable {
                    index: idx,
                    field: "base",
                    value: f.value.clone(),
                })?;
                bases.push((idx, v));
            }
            Some((MemField::Size, idx)) => {
                let v = parse_size(&f.value, &f.unit).ok_or_else(|| MemoryError::Unparsable {
                    index: idx,
                    field: "size",
                    value: format!("{}{}", f.value, f.unit),
                })?;
                if v == 0 {
                    return Err(MemoryError::ZeroSize { index: idx });
                }
                sizes.push((idx, v));
            }
            None => {}
        }
    }
    let mut spans = Vec::new();
    for (idx, base) in &bases {
        let size = sizes
            .iter()
            .find(|(i, _)| i == idx)
            .map(|(_, s)| *s)
            .ok_or(MemoryError::UnpairedIndex {
                index: *idx,
                missing: "size",
            })?;
        spans.push(MemorySpan {
            index: *idx,
            base: *base,
            size,
        });
    }
    for (idx, _) in &sizes {
        if !bases.iter().any(|(i, _)| i == idx) {
            return Err(MemoryError::UnpairedIndex {
                index: *idx,
                missing: "base",
            });
        }
    }
    spans.sort_by_key(|s| (s.base, s.index));
    for (i, a) in spans.iter().enumerate() {
        for b in spans.iter().skip(i + 1) {
            if a.overlaps(b) {
                return Err(MemoryError::Overlap {
                    a: a.index,
                    b: b.index,
                });
            }
        }
    }
    Ok(spans)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemField {
    Base,
    Size,
}

/// 解析 `base[i]` / `size[i]`。**仅**带索引后缀的键参与窗口提取：
/// 无索引的 `base`/`size`（单位 `none` 的字面记录）不是窗口事实。
///
/// 复查修复（70.14）：字段名**去空白 + ASCII 小写归一**、索引去空白。旧实现
/// 要求键逐字节等于 `base[i]`/`size[i]`，于是 `base[0] `（尾随空格）、
/// `base [0]`（内嵌空格）、`Base[0]`（大小写）**都不被提取**——这些键既不触发
/// 重叠/成对判据，也不与规范键判重（`normalize_key` 只折叠空白、不小写），
/// RAM 窗口判据（蓝图 C3"错 RAM 重叠使准入红"）可被键格式静默规避。
/// 归一后任何形态的窗口键进同一判据。
fn parse_mem_key(key: &str) -> Option<(MemField, u32)> {
    let (field, rest) = key.split_once('[')?;
    // 先 trim 再剥 `]`：`base[0] `（尾随空白）否则连 `]` 都剥不掉。
    let idx: u32 = rest.trim().strip_suffix(']')?.trim().parse().ok()?;
    let field = field.trim();
    let field = if field.eq_ignore_ascii_case("base") {
        MemField::Base
    } else if field.eq_ignore_ascii_case("size") {
        MemField::Size
    } else {
        return None;
    };
    Some((field, idx))
}

/// 地址解析：`0x...` / `0X...` / 十进制；未冻结占位（[`MEM_ADDR_UNKNOWN`]）拒绝。
pub fn parse_addr(value: &str) -> Option<u64> {
    let v = value.trim();
    if v.is_empty() || v.eq_ignore_ascii_case(MEM_ADDR_UNKNOWN) {
        return None;
    }
    let (body, radix) = match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(hex) => (hex, 16),
        None => (v, 10),
    };
    u64::from_str_radix(body, radix).ok()
}

/// 容量解析：数值 + 单位（`B`/`KiB`/`MiB`/`GiB`/`TiB`；`KB/MB/GB/TB` 按
/// 二进制口径接受但**记录口径差异**由来源串负责）。
pub fn parse_size(value: &str, unit: &str) -> Option<u64> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    let n: u64 = v.parse().ok()?;
    let shift = match unit.trim() {
        "B" | "bytes" => 0,
        "KiB" | "KB" => 10,
        "MiB" | "MB" => 20,
        "GiB" | "GB" => 30,
        "TiB" | "TB" => 40,
        _ => return None,
    };
    // 审计（S7 F12）：`checked_shl` 只拒绝移位数 ≥64，**不拒绝高位移出**——
    // `16777217 TiB` 会被读成 1 TiB。改用 checked_mul。
    n.checked_mul(1u64.checked_shl(shift)?)
}

/// 便捷构造：已验证 RAM 窗口事实对（迁移/测试用）。
pub fn ram_window_facts(index: u32, base: u64, size_mib: u64) -> [Fact; 2] {
    let mut b = Fact::repo(
        Subsystem::Ram,
        &format!("base[{index}]"),
        &format!("0x{base:x}"),
        "addr",
    );
    b.source = "test harness".to_string();
    let mut s = Fact::repo(
        Subsystem::Ram,
        &format!("size[{index}]"),
        &size_mib.to_string(),
        "MiB",
    );
    s.source = "test harness".to_string();
    [b, s]
}

/// 未冻结 RAM 占位事实（`verified=false`，值 `unknown`）。
pub fn ram_window_unknown(index: u32) -> Fact {
    let mut f = Fact::unknown(Subsystem::Ram, &format!("base[{index}]"));
    f.value = MEM_ADDR_UNKNOWN.to_string();
    f
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test]
    fn paired_windows_extract_and_sort() {
        let mut facts = Vec::new();
        facts.extend(ram_window_facts(1, 0x8000_0000, 1024));
        facts.extend(ram_window_facts(0, 0x4000_0000, 512));
        let spans = spans_from_facts(&facts).expect("no overlap");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].base, 0x4000_0000);
        assert_eq!(spans[1].end(), 0x8000_0000 + 1024 * 1024 * 1024);
    }

    #[test]
    fn overlap_is_rejected() {
        // 0x0..0x8000_0000 与 0x4000_0000..0x4400_0000 重叠。
        let mut facts = Vec::new();
        facts.extend(ram_window_facts(0, 0x0, 2048));
        facts.extend(ram_window_facts(1, 0x4000_0000, 64));
        assert_eq!(
            spans_from_facts(&facts),
            Err(MemoryError::Overlap { a: 0, b: 1 })
        );
    }

    #[test]
    fn adjacent_windows_do_not_overlap() {
        let mut facts = Vec::new();
        facts.extend(ram_window_facts(0, 0x4000_0000, 1024));
        facts.extend(ram_window_facts(1, 0x8000_0000, 1024));
        assert!(spans_from_facts(&facts).is_ok(), "首尾相接不算重叠");
    }

    #[test]
    fn verified_base_without_size_is_rejected() {
        let mut facts = Vec::new();
        facts.extend(ram_window_facts(0, 0x4000_0000, 1024));
        facts.retain(|f| f.key == "base[0]");
        assert_eq!(
            spans_from_facts(&facts),
            Err(MemoryError::UnpairedIndex {
                index: 0,
                missing: "size"
            })
        );
    }

    #[test]
    fn unverified_windows_are_not_platform_facts() {
        // 未冻结窗口（unknown 占位）不进重叠判定，也不报错。
        let facts = vec![ram_window_unknown(0)];
        assert_eq!(spans_from_facts(&facts), Ok(Vec::new()));
    }

    #[test]
    fn zero_and_unparsable_sizes_are_rejected() {
        let mut facts = Vec::new();
        let [mut b, mut s] = ram_window_facts(0, 0x4000_0000, 1024);
        s.value = "0".into();
        b.source = "harness".into();
        facts.push(b);
        facts.push(s);
        assert_eq!(
            spans_from_facts(&facts),
            Err(MemoryError::ZeroSize { index: 0 })
        );
        let mut facts = Vec::new();
        let [b, mut s] = ram_window_facts(0, 0x4000_0000, 1024);
        s.value = "big".into();
        facts.push(b);
        facts.push(s);
        assert!(matches!(
            spans_from_facts(&facts),
            Err(MemoryError::Unparsable { .. })
        ));
        // 未冻结的 `unknown` 基址（键带索引）拒绝——不得当作"没记录"。
        let mut facts = Vec::new();
        let [mut b, s] = ram_window_facts(0, 0x4000_0000, 1024);
        b.value = MEM_ADDR_UNKNOWN.into();
        facts.push(b);
        facts.push(s);
        assert!(matches!(
            spans_from_facts(&facts),
            Err(MemoryError::Unparsable { field: "base", .. })
        ));
    }

    #[test]
    fn size_units_are_binary_documented() {
        assert_eq!(parse_size("1", "GiB"), Some(1 << 30));
        assert_eq!(parse_size("2", "MiB"), Some(2 << 20));
        assert_eq!(parse_size("3", "B"), Some(3));
        assert_eq!(parse_size("1", "furlong"), None, "未知单位拒绝，不猜");
        assert_eq!(parse_size("", "GiB"), None);
    }

    /// 70.14 复查红测：窗口键的**空白/大小写变体**不得绕过重叠判据。
    ///
    /// 缺陷形态（旧实现）：`parse_mem_key` 要求键逐字节等于 `base[i]`/`size[i]`，
    /// 于是 `base[0] `（尾随空格）、`base [0]`（内嵌空格）、`Base[0]`（大小写）
    /// 都不被提取 ⇒ 这些"疑似窗口"事实既不进重叠/成对判据，也不与规范键判重
    /// （`normalize_key` 只折叠空白、不小写）——RAM 窗口判据（蓝图 C3）可被
    /// 键格式静默规避。归一后必须与规范形态**同判**。
    #[test]
    fn window_key_variants_do_not_escape_overlap_judgement() {
        for variant in ["base[0] ", "base [0]", "Base[0]", " BASE[0] "] {
            let mut facts = Vec::new();
            let [mut b, s] = ram_window_facts(0, 0x0, 2048);
            b.key = variant.to_string();
            facts.push(b);
            facts.push(s);
            // 第二个窗口与第一个重叠 → 必须报 Overlap（旧实现返回 Ok）。
            let [b1, s1] = ram_window_facts(1, 0x4000_0000, 64);
            facts.push(b1);
            facts.push(s1);
            let got = spans_from_facts(&facts);
            assert!(
                matches!(got, Err(MemoryError::Overlap { .. })),
                "键变体 {variant:?} 必须参与重叠判据，实得 {got:?}"
            );
        }
        // 大小写变体同样进成对判据（缺 size 必须报 UnpairedIndex）。
        let [mut b, _s] = ram_window_facts(7, 0x1000, 1);
        b.key = "Base[7]".to_string();
        assert!(
            matches!(
                spans_from_facts(&[b]),
                Err(MemoryError::UnpairedIndex { index: 7, .. })
            ),
            "大小写变体必须被提取并报成对缺失"
        );
    }
}
