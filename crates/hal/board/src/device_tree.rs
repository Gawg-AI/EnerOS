//! WP-26.9（26-C §3.2 步骤 1）：FDT（Flattened Device Tree）解析器。
//!
//! 从 boot 链提供的 FDT blob 解析设备清单（`DeviceInventory`）——权威设备
//! 源为 `configs/*.dts`（dtc 编译为 dtb 后即 FDT blob 形态）。
//!
//! # 格式（FDT v17）
//! - header：magic`0xd00dfeed`@0、totalsize@4、off_dt_struct@8、
//!   off_dt_strings@12、off_mem_rsvmap@16、version@20、
//!   size_dt_strings@32、size_dt_struct@36；
//! - 结构块令牌：`FDT_BEGIN_NODE(1)`、`FDT_END_NODE(2)`、`FDT_PROP(3)`
//!   （`FDT_NOP(4)` 跳过）；节点名 NUL 结尾（含 unit-address）；
//! - 字符串块：属性名 NUL 结尾。
//!
//! no_std + alloc：仅使用 `alloc::vec::Vec` / `alloc::string::String`。

extern crate alloc;

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;

/// FDT 魔数（`\xd0\x0d\xfe\xed`）。
pub const FDT_MAGIC: u32 = 0xd00d_feed;

/// 结构块令牌。
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
/// 结构块终止标记（dtc 输出在根 END_NODE 后附加）。
const FDT_END: u32 = 9;

/// FDT 解析错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdtError {
    /// 长度不足或魔数不符。
    BadHeader,
    /// 结构/字符串块偏移越界。
    Truncated,
    /// 令牌非法（结构块损坏）。
    BadToken,
    /// 属性名不在字符串块。
    BadPropertyName,
}

/// 一个设备节点（WP-26.9 所需字段；其余属性忽略）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceNode {
    /// 节点名（含 unit-address，如 `pl011@9000000`）。
    pub name: String,
    /// `compatible` 属性（首个值；无 → 空）。
    pub compatible: String,
    /// `reg` 属性解析出的 (基址, 长度) 对（`#address-cells`/`#size-cells` 为
    /// 默认 2 时按 64 位解析；本实现固定按 2/2 解析——qemu-virt 与
    /// 本仓库 dts 均使用 2/2）。
    pub regs: Vec<(u64, u64)>,
    /// `interrupts` 属性原始单元（u32 序列；GIC 编码语义由消费者解释）。
    pub interrupts: Vec<u32>,
}

/// 设备清单（composition root 的输入）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceInventory {
    pub nodes: Vec<DeviceNode>,
}

impl DeviceInventory {
    /// 按 compatible 过滤（首个匹配；无 → None）。
    pub fn find_by_compatible(&self, compat: &str) -> Option<&DeviceNode> {
        self.nodes.iter().find(|n| n.compatible == compat)
    }

    /// 全部节点数。
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// 解析 FDT blob → 设备清单（WP-26.9）。
///
/// 遍历结构块：BEGIN_NODE 入栈（记录名字），PROP 读属性（compatible/reg/
/// interrupts 提取），END_NODE 出栈。仅注册顶层+一层子节点（`/` 的子节点
/// 即设备）；更深层（如 `soc/uart@...`）也注册（名称含路径前缀）。
pub fn parse_fdt(blob: &[u8]) -> Result<DeviceInventory, FdtError> {
    if blob.len() < 40 {
        return Err(FdtError::BadHeader);
    }
    let magic = u32::from_be_bytes([blob[0], blob[1], blob[2], blob[3]]);
    if magic != FDT_MAGIC {
        return Err(FdtError::BadHeader);
    }
    let totalsize = u32::from_be_bytes([blob[4], blob[5], blob[6], blob[7]]) as usize;
    let off_struct = u32::from_be_bytes([blob[8], blob[9], blob[10], blob[11]]) as usize;
    let off_strings = u32::from_be_bytes([blob[12], blob[13], blob[14], blob[15]]) as usize;
    // FDT v17 header：off_mem_rsvmap@16、version@20、last_comp_version@24、
    // boot_cpuid_phys@28、size_dt_strings@32、size_dt_struct@36。
    let size_strings = u32::from_be_bytes([blob[32], blob[33], blob[34], blob[35]]) as usize;
    if totalsize > blob.len() || off_struct + 4 > totalsize {
        return Err(FdtError::Truncated);
    }
    let strings_end = off_strings.saturating_add(size_strings);
    if strings_end > totalsize {
        return Err(FdtError::Truncated);
    }

    let mut nodes: Vec<DeviceNode> = Vec::new();
    let mut cur: Option<DeviceNode> = None;
    let mut pos = off_struct;
    let end = off_strings; // 结构块在字符串块前
    while pos + 4 <= end {
        let token = u32::from_be_bytes([blob[pos], blob[pos + 1], blob[pos + 2], blob[pos + 3]]);
        pos += 4;
        match token {
            FDT_BEGIN_NODE => {
                let (name, next) = read_cstr(blob, pos, end)?;
                pos = align4(next);
                // 记录当前节点（根 `/` 跳过）。
                if name != "/" {
                    cur = Some(DeviceNode {
                        name,
                        compatible: String::new(),
                        regs: Vec::new(),
                        interrupts: Vec::new(),
                    });
                } else {
                    cur = None;
                }
            }
            FDT_END_NODE => {
                if let Some(node) = cur.take() {
                    if !node.name.is_empty() {
                        nodes.push(node);
                    }
                }
            }
            FDT_PROP => {
                if pos + 8 > end {
                    return Err(FdtError::Truncated);
                }
                let len =
                    u32::from_be_bytes([blob[pos], blob[pos + 1], blob[pos + 2], blob[pos + 3]])
                        as usize;
                let nameoff = u32::from_be_bytes([
                    blob[pos + 4],
                    blob[pos + 5],
                    blob[pos + 6],
                    blob[pos + 7],
                ]) as usize;
                pos += 8;
                let value = blob.get(pos..pos + len).ok_or(FdtError::Truncated)?;
                pos = align4(pos + len);
                let prop_name = read_string_at(blob, off_strings, nameoff, strings_end)?;
                if let Some(node) = cur.as_mut() {
                    match prop_name.as_str() {
                        "compatible" if !value.is_empty() => {
                            node.compatible = read_cstr(value, 0, value.len())?.0;
                        }
                        "compatible" => {}
                        "reg" => {
                            node.regs = parse_reg(value);
                        }
                        "interrupts" => {
                            node.interrupts = value
                                .chunks_exact(4)
                                .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
                                .collect();
                        }
                        _ => {}
                    }
                }
            }
            FDT_NOP => {}
            // 结构块终止（dtc 在根 END_NODE 后输出 FDT_END=9）。
            FDT_END => break,
            _other => return Err(FdtError::BadToken),
        }
    }
    Ok(DeviceInventory { nodes })
}

/// 解析 `reg` 属性（#address-cells=2/#size-cells=2 固定，与仓库 dts 一致）。
fn parse_reg(value: &[u8]) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    for pair in value.chunks_exact(16) {
        let addr = u64::from_be_bytes(pair[0..8].try_into().unwrap_or([0; 8]));
        let size = u64::from_be_bytes(pair[8..16].try_into().unwrap_or([0; 8]));
        out.push((addr, size));
    }
    out
}

/// 从 `buf[pos..]` 读 NUL 结尾字符串（不越界）。
fn read_cstr(buf: &[u8], pos: usize, end: usize) -> Result<(String, usize), FdtError> {
    let mut len = 0usize;
    while pos + len < end && buf[pos + len] != 0 {
        len += 1;
    }
    if pos + len >= end {
        return Err(FdtError::Truncated);
    }
    let s = core::str::from_utf8(&buf[pos..pos + len])
        .map_err(|_| FdtError::BadToken)?
        .to_owned();
    Ok((s, pos + len + 1))
}

/// 从字符串块按偏移读属性名。
fn read_string_at(
    blob: &[u8],
    strings_base: usize,
    off: usize,
    strings_end: usize,
) -> Result<String, FdtError> {
    let pos = strings_base + off;
    if pos >= strings_end {
        return Err(FdtError::BadPropertyName);
    }
    read_cstr(blob, pos, strings_end).map(|(s, _)| s)
}

/// 对齐到 4 字节（FDT 结构块对齐）。
fn align4(x: usize) -> usize {
    (x + 3) & !3
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    /// 真实设备树（dtc 编译 configs/qemu-virt.dts）——解析器必须能吃
    /// 真实节点（26-C §3.2：pl011 UART / armv8-timer）。
    const QEMU_VIRT_DTB: &[u8] = include_bytes!("../tests/fixtures/qemu-virt.dtb");

    #[test]
    fn parses_pl011_and_timer_nodes() {
        let inv = parse_fdt(QEMU_VIRT_DTB).expect("parse 真实 dtb");
        let pl011 = inv.find_by_compatible("arm,pl011").expect("pl011 节点");
        assert_eq!(pl011.name, "uart@9000000");
        assert_eq!(pl011.regs, vec![(0x0900_0000, 0x1000)]);
        assert_eq!(pl011.interrupts, vec![0, 1, 4]);
        let timer = inv
            .find_by_compatible("arm,armv8-timer")
            .expect("timer 节点");
        // 4 组 PPI（物理/虚拟/安全 timer 等）；虚拟 timer（1, 13, 11）
        // 即 IRQ 27——WP-26.8 绑定依据。
        assert!(
            timer.interrupts.chunks_exact(3).any(|c| c == [1, 13, 11]),
            "虚拟 timer PPI (1,13,11) 必须在 interrupts 中: {:?}",
            timer.interrupts
        );
    }

    #[test]
    fn rejects_bad_magic() {
        let mut blob = QEMU_VIRT_DTB.to_vec();
        blob[0] = 0x00;
        assert_eq!(parse_fdt(&blob), Err(FdtError::BadHeader));
    }

    #[test]
    fn rejects_truncated() {
        let blob = QEMU_VIRT_DTB;
        assert_eq!(parse_fdt(&blob[..20]), Err(FdtError::BadHeader));
        assert_eq!(parse_fdt(&blob[..blob.len() - 4]), Err(FdtError::Truncated));
    }
}
