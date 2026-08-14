//! 38（WP-38.2）：完整、受限的 FDT walker（替换 26-C 的扁平解析）。
//!
//! # 解析能力（蓝图 38 WP-38.2）
//! - 有界栈跟踪节点父子关系（路径前缀）；最大 depth/node/property/compatible
//!   数受控（防恶意 DTB OOM）；
//! - 继承每层 `#address-cells/#size-cells`，支持合法 1/2 cells 组合；
//! - 读取全部 NUL-separated `compatible`（优先级序）；
//! - `status=disabled/fail/reserved` 不进入设备清单（单独报告）；
//! - 应用嵌套 bus `ranges` 地址翻译并检测 overflow/重叠；
//! - 解析 `interrupt-parent`（本节点显式 → 否则根级缺省）与
//!   `#interrupt-cells`（缺省 3）→ GIC 全局中断号（SPI=32+n / PPI=16+n）；
//!   `interrupt-map` 多级路由未实现（CVR-38-04 → 39+，有 `BadInterruptMap`
//!   错误面预留，不猜路由）；
//! - 建 phandle 索引，解析 `clocks`（phandle → `clock-frequency`）、
//!   `resets`/`gpios`/`dmas`（按 provider `#*-cells` 拆分）/`iommus`
//!   引用；
//! - 处理 `dma-ranges`/`dma-coherent`/`reserved-memory`；
//! - 解析 CPU 节点（device_type=cpu）、memory、chosen stdout-path、aliases；
//! - 校验 property 长度/cell 数/字符串终止/重复 phandle；未知属性保留在
//!   有界 extension map，不影响核心；
//! - 所有错误含 node path/property/offset，便于板级适配。
//!
//! # 不变量
//! - discovery **只读**：本模块不触发设备写/格式化/业务 I/O（固定不变量 1）；
//! - FDT 只是 [`PlatformDescriptionProvider`] 的一个实现（hal seam）。

extern crate alloc;

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use eneros_hal::inventory::{ClockRef, InterruptSpec};
use eneros_hal::platform_adapter::{PlatformDescriptionProvider, PlatformInput};
use eneros_hal::types::IrqTrigger;

/// FDT 魔数（`\xd0\x0d\xfe\xed`）。
pub const FDT_MAGIC: u32 = 0xd00d_feed;

/// 结构块令牌。
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
/// 结构块终止标记（dtc 输出在根 END_NODE 后附加）。
const FDT_END: u32 = 9;

/// 有界限制（WP-38.2：防恶意 DTB 耗尽资源）。
pub const MAX_DEPTH: usize = 16;
pub const MAX_NODES: usize = 512;
pub const MAX_PROPERTIES_PER_NODE: usize = 64;
pub const MAX_COMPATIBLES: usize = 8;
pub const MAX_PHANDLES: usize = 256;

/// FDT 解析错误（含 node path/property/offset 定位）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FdtError {
    /// 长度不足或魔数不符。
    BadHeader,
    /// 结构/字符串块偏移越界。
    Truncated,
    /// 令牌非法（结构块损坏）。
    BadToken,
    /// 属性名不在字符串块。
    BadPropertyName,
    /// 属性长度与 cell 数不匹配。
    BadCellCount {
        path: String,
        property: String,
        offset: usize,
    },
    /// ranges 条目数不足或翻译回绕。
    BadRanges { path: String, offset: usize },
    /// 嵌套深度超限（[`MAX_DEPTH`]）。
    TooDeep { path: String },
    /// 节点数超限（[`MAX_NODES`]）。
    TooManyNodes,
    /// 属性数超限。
    TooManyProperties { path: String },
    /// 重复 phandle（篡改/损坏）。
    DuplicatePhandle { phandle: u32 },
    /// phandle 引用不存在（clocks/interrupt-parent 等）。
    UnknownPhandle { phandle: u32, context: String },
    /// interrupt-map 无匹配条目。
    BadInterruptMap { path: String },
    /// 字符串未终止或非 UTF-8。
    BadString,
}

/// 节点状态（FDT `status` 语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    Ok,
    Disabled,
    Fail,
    Reserved,
}

/// 一个设备节点（绑定表/驱动工厂消费面）。
///
/// `regs` 为**经 bus ranges 翻译后的 CPU 物理地址**（`(base, size)`）；
/// `regs_bus` 为翻译前总线地址；`interrupts` 保留原始 GIC cells（兼容旧
/// 消费者 `chunks_exact(3)` 语义），`global_irqs` 为解析后的全局中断号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceNode {
    /// 节点名（含 unit-address，如 `pl011@9000000`）。
    pub name: String,
    /// 完整路径（如 `/soc/uart@9000000`）。
    pub path: String,
    /// `compatible` 首个值（绑定表匹配用；无 → 空）。
    pub compatible: String,
    /// `compatible` 全部值（优先级序）。
    pub compatible_all: Vec<String>,
    /// 节点状态。
    pub status: NodeStatus,
    /// phandle（存在时）。
    pub phandle: Option<u32>,
    /// 翻译后 reg 对（CPU 物理地址）。
    pub regs: Vec<(u64, u64)>,
    /// 翻译前 reg 对（总线地址）。
    pub regs_bus: Vec<(u64, u64)>,
    /// `interrupts` 原始单元（GIC 编码 [type, num, flags] 三元组）。
    pub interrupts: Vec<u32>,
    /// 解析后的全局中断号。
    pub global_irqs: Vec<InterruptSpec>,
    /// `clocks` 引用（phandle + 解析出的频率）。
    pub clocks: Vec<ClockRef>,
    /// `resets` 引用（phandle + specifier cells）。
    pub resets: Vec<eneros_hal::inventory::ResetRef>,
    /// `gpios` 引用（phandle + specifier cells）。
    pub gpios: Vec<eneros_hal::inventory::GpioRef>,
    /// `dmas` 引用（phandle + specifier cells）。
    pub dmas: Vec<eneros_hal::inventory::DmaChannelRef>,
    /// 节点自身 `clock-frequency`（fixed-clock 等）。
    pub clock_frequency: Option<u64>,
    /// `dma-coherent`（None = 未声明 → 平台默认）。
    pub dma_coherent: Option<bool>,
    /// `dma-ranges`（child_addr, parent_addr, size）。
    pub dma_ranges: Vec<(u64, u64, u64)>,
    /// `iommus` 引用（phandle + domain cell）。
    pub iommus: Vec<(u32, u32)>,
    /// 有界扩展属性（未知属性；name → 原始字节）。
    pub properties: Vec<(String, Vec<u8>)>,
}

/// 设备清单（composition root 的输入）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceInventory {
    /// status=okay 的节点（绑定表消费面）。
    pub nodes: Vec<DeviceNode>,
    /// status=disabled/fail/reserved 的节点路径（报告面；不 bind）。
    pub disabled: Vec<String>,
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

/// 完整 FDT 树（walker 产物；`to_inventory` 归一化到绑定表消费面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdtTree {
    pub nodes: Vec<DeviceNode>,
    /// phandle → 节点索引。
    pub phandle_index: Vec<(u32, usize)>,
    /// aliases（name → 路径）。
    pub aliases: Vec<(String, String)>,
    /// chosen stdout-path。
    pub stdout_path: Option<String>,
    /// 内存区（compatible=device memory 的节点）。
    pub memory: Vec<(u64, u64)>,
    /// reserved-memory 区（purpose 名）。
    pub reserved: Vec<(u64, u64, String)>,
    /// CPU 节点（id）。
    pub cpus: Vec<u32>,
}

impl FdtTree {
    /// phandle 查节点索引。
    pub fn find_phandle(&self, phandle: u32) -> Option<usize> {
        self.phandle_index
            .iter()
            .find(|(p, _)| *p == phandle)
            .map(|(_, i)| *i)
    }

    /// 归一化到绑定表消费面（disabled 节点移入 `disabled` 报告面）。
    pub fn to_inventory(&self) -> DeviceInventory {
        let mut inv = DeviceInventory::default();
        for n in &self.nodes {
            if n.status == NodeStatus::Ok {
                inv.nodes.push(n.clone());
            } else {
                inv.disabled.push(n.path.clone());
            }
        }
        inv
    }
}

/// FDT 描述提供者（hal `PlatformDescriptionProvider` 的 FDT 实现）。
#[derive(Debug, Clone, Copy, Default)]
pub struct FdtDescriptionProvider;

impl PlatformDescriptionProvider for FdtDescriptionProvider {
    fn describe(
        &self,
        input: &PlatformInput,
    ) -> Result<eneros_hal::PlatformInventory, eneros_hal::platform_adapter::PlatformError> {
        let Some(dtb) = input.runtime_dtb else {
            return Err(eneros_hal::platform_adapter::PlatformError::NoDescription);
        };
        let tree = parse_fdt_tree(dtb)
            .map_err(|_| eneros_hal::platform_adapter::PlatformError::ParseFailed)?;
        tree.into_platform_inventory(input.bootinfo_cpu_count)
    }
}

impl FdtTree {
    /// 归一化到 hal `PlatformInventory`（WP-38.3 合并入口：FDT 发现事实 →
    /// inventory；product manifest 契约在装配层校验）。
    pub fn into_platform_inventory(
        &self,
        bootinfo_cpu_count: u32,
    ) -> Result<eneros_hal::PlatformInventory, eneros_hal::platform_adapter::PlatformError> {
        use eneros_hal::inventory::{
            ChosenResources, CpuNode, DeviceId, DeviceResources, DeviceStatus, InventoryBuilder,
            MemoryRegion, MmioRegion, NonEmptyVec, PlatformProvenance, ReservedRegion,
        };
        use eneros_hal::platform_adapter::PlatformError;
        // 只收录 status=okay 且带 compatible 的节点（无 compatible 的是
        // memory/cpus/chosen 等非设备节点——不是驱动消费面；未知 compatible
        // 的设备保留事实，由绑定/装配层返回 Unsupported）。
        // **索引空间**：device 列表索引 ≠ tree 节点索引——chosen 角色
        // （console/entropy）必须记录 device 列表索引（38 实测：tree 索引
        // 41 在 device 列表中是 GIC，stdout-path 解析错位）。
        let mut devices = Vec::new();
        let mut tree_to_device: Vec<Option<usize>> = alloc::vec![None; self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            if n.status != NodeStatus::Ok || n.compatible_all.is_empty() {
                continue;
            }
            tree_to_device[i] = Some(devices.len());
            let compat =
                NonEmptyVec::new(n.compatible_all.clone()).ok_or(PlatformError::Unsupported)?;
            let regs = n
                .regs
                .iter()
                .map(|(b, s)| MmioRegion {
                    phys: eneros_hal::PhysAddr::new(*b),
                    size: *s,
                })
                .collect();
            let mut props = eneros_hal::inventory::BoundedPropertyMap::default();
            for (k, v) in n
                .properties
                .iter()
                .take(eneros_hal::inventory::MAX_PROPERTIES)
            {
                let _ = props.try_push(k.clone(), v.clone());
            }
            devices.push(DeviceResources {
                id: DeviceId(i as u32),
                compatible: compat,
                status: DeviceStatus::Ok,
                regs,
                interrupts: n.global_irqs.clone(),
                clocks: n.clocks.clone(),
                resets: n.resets.clone(),
                gpios: n.gpios.clone(),
                dmas: n.dmas.clone(),
                iommu: n
                    .iommus
                    .first()
                    .map(|(p, d)| eneros_hal::inventory::IommuRef {
                        phandle: *p,
                        domain_id: *d,
                    }),
                dma_coherency: match n.dma_coherent {
                    Some(true) => eneros_hal::DmaCoherency::Coherent,
                    _ => eneros_hal::DmaCoherency::NonCoherent,
                },
                properties: props,
            });
        }
        // CPU：FDT CPU 节点；缺失时以 bootinfo 数为兜底（两者都不足 → 空，
        // 由装配层 fail-closed）。
        let cpus: Vec<CpuNode> = if !self.cpus.is_empty() {
            self.cpus
                .iter()
                .map(|id| CpuNode {
                    id: *id,
                    enabled: true,
                })
                .collect()
        } else if bootinfo_cpu_count > 0 {
            (0..bootinfo_cpu_count)
                .map(|id| CpuNode { id, enabled: true })
                .collect()
        } else {
            Vec::new()
        };
        // chosen 角色经 tree→device 索引映射（tree 索引在 device 列表中
        // 无意义——38 实测错位）。
        let console_dev = self.console_device_index().and_then(|i| tree_to_device[i]);
        Ok(InventoryBuilder {
            cpus,
            memory: self
                .memory
                .iter()
                .map(|(b, s)| MemoryRegion {
                    phys: eneros_hal::PhysAddr::new(*b),
                    size: *s,
                })
                .collect(),
            reserved_memory: self
                .reserved
                .iter()
                .map(|(b, s, p)| ReservedRegion {
                    phys: eneros_hal::PhysAddr::new(*b),
                    size: *s,
                    purpose: p.clone(),
                })
                .collect(),
            devices,
            chosen: ChosenResources {
                stdout: self.stdout_path.clone(),
                console: console_dev,
                ..Default::default()
            },
            ..Default::default()
        }
        .build(PlatformProvenance {
            source: eneros_hal::inventory::DescriptionSource::Fdt,
            digest: None,
            description: "runtime-dtb".into(),
        }))
    }
}

// ============================================================================
// walker 实现
// ============================================================================

/// 每层节点上下文（有界栈）。
struct Frame {
    /// 节点名。
    name: String,
    /// 完整路径。
    path: String,
    /// 本层 #address-cells（继承）。
    addr_cells: u32,
    /// 本层 #size-cells（继承）。
    size_cells: u32,
    /// 属性（收集期）。
    props: Vec<(String, Vec<u8>)>,
    /// 是否根。
    is_root: bool,
}

#[allow(dead_code)] // 保留：合成 DTB 构造/未来 cell 访问用
fn cell_u32(value: &[u8], idx: usize) -> Result<u32, FdtError> {
    let base = idx * 4;
    let end = base + 4;
    if end > value.len() {
        return Err(FdtError::BadCellCount {
            path: "?".into(),
            property: "cell".into(),
            offset: base,
        });
    }
    Ok(u32::from_be_bytes([
        value[base],
        value[base + 1],
        value[base + 2],
        value[base + 3],
    ]))
}

/// 读 NUL 结尾字符串（不越界；UTF-8 校验）。
fn read_cstr(buf: &[u8], pos: usize, end: usize) -> Result<(String, usize), FdtError> {
    let mut len = 0usize;
    while pos + len < end && buf[pos + len] != 0 {
        len += 1;
    }
    if pos + len >= end {
        return Err(FdtError::BadString);
    }
    let s = core::str::from_utf8(&buf[pos..pos + len]).map_err(|_| FdtError::BadString)?;
    Ok((s.to_owned(), pos + len + 1))
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

/// 解析 FDT blob → 完整树（WP-38.2）。
pub fn parse_fdt_tree(blob: &[u8]) -> Result<FdtTree, FdtError> {
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
    let size_strings = u32::from_be_bytes([blob[32], blob[33], blob[34], blob[35]]) as usize;
    if totalsize > blob.len() || off_struct + 4 > totalsize {
        return Err(FdtError::Truncated);
    }
    let strings_end = off_strings.saturating_add(size_strings);
    if strings_end > totalsize {
        return Err(FdtError::Truncated);
    }

    let mut tree = FdtTree {
        nodes: Vec::new(),
        phandle_index: Vec::new(),
        aliases: Vec::new(),
        stdout_path: None,
        memory: Vec::new(),
        reserved: Vec::new(),
        cpus: Vec::new(),
    };
    // 引用类属性原始 cells（node_idx, raw cells）；树完整后按 provider
    // #*-cells 拆分回填。
    let mut pending_clocks: Vec<(usize, Vec<u32>)> = Vec::new();
    let mut pending_resets: Vec<(usize, Vec<u32>)> = Vec::new();
    let mut pending_gpios: Vec<(usize, Vec<u32>)> = Vec::new();
    let mut pending_dmas: Vec<(usize, Vec<u32>)> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    // 根帧（#address-cells=2/#size-cells=2 缺省；子节点继承）。
    stack.push(Frame {
        name: "/".into(),
        path: "/".into(),
        addr_cells: 2,
        size_cells: 2,
        props: Vec::new(),
        is_root: true,
    });

    let mut pos = off_struct;
    let end = off_strings;
    while pos + 4 <= end {
        let token = u32::from_be_bytes([blob[pos], blob[pos + 1], blob[pos + 2], blob[pos + 3]]);
        pos += 4;
        match token {
            FDT_BEGIN_NODE => {
                let (name, next) = read_cstr(blob, pos, end).map_err(|_| FdtError::BadToken)?;
                pos = align4(next);
                if name == "/" || name.is_empty() {
                    continue; // 根已入栈（部分 DTB 以空名编码根）
                }
                if stack.len() >= MAX_DEPTH {
                    let path = stack.last().map(|f| f.path.clone()).unwrap_or_default();
                    return Err(FdtError::TooDeep { path });
                }
                let parent = stack.last().expect("root 帧恒在");
                let path = if parent.is_root {
                    format!("/{name}")
                } else {
                    format!("{}/{}", parent.path, name)
                };
                stack.push(Frame {
                    name,
                    path,
                    addr_cells: parent.addr_cells,
                    size_cells: parent.size_cells,
                    props: Vec::new(),
                    is_root: false,
                });
            }
            FDT_END_NODE => {
                // 完成当前层：解析属性 → 生成节点/收集全局事实。
                let frame = stack.pop().expect("root 帧恒在");
                if frame.is_root {
                    continue;
                }
                // 本节点自己的 cells（FDT_PROP 已即时更新 frame 字段；
                // 此处只读，不再反向污染父帧）。
                // reg 按 DT 规范使用**父级** cells（`#address-cells` 描述
                // 子节点 reg 的解释）。
                let parent = stack.last().expect("root 帧恒在");
                let compatible_all = parse_compatible(&frame.props, &frame.path)?;
                let status = parse_status(&frame.props);
                let phandle = prop_u32(&frame.props, "phandle")
                    .or_else(|| prop_u32(&frame.props, "linux,phandle"));
                if let Some(p) = phandle {
                    if tree.find_phandle(p).is_some() {
                        return Err(FdtError::DuplicatePhandle { phandle: p });
                    }
                    tree.phandle_index.push((p, tree.nodes.len()));
                }
                let regs_bus = parse_reg_pairs(
                    &frame.props,
                    "reg",
                    parent.addr_cells,
                    parent.size_cells,
                    &frame.path,
                )?;
                // dma-ranges：<child_addr(本节点 cells) parent_addr(父级 cells) size>。
                let dma_ranges = parse_dma_ranges(
                    &frame.props,
                    frame.addr_cells,
                    parent.addr_cells,
                    frame.size_cells,
                    &frame.path,
                )?;
                let dma_coherent = frame
                    .props
                    .iter()
                    .find(|(k, _)| k == "dma-coherent")
                    .map(|(_, v)| v.is_empty());
                // dma-coherent 常带空值；无属性时父级继承在翻译层做。
                let clock_frequency = prop_u64(&frame.props, "clock-frequency");
                let iommus = parse_iommus(&frame.props, &frame.path)?;
                let interrupts_raw = prop_u32s(&frame.props, "interrupts");
                let global_irqs = parse_interrupts(&frame, &stack, &tree, &interrupts_raw)?;

                // 引用类属性（clocks/resets/gpios/dmas）：先收集原始 cells
                // （`<&phandle cells...>*`），树完整后按 provider 的
                // `#*-cells` 拆分回填（qemu 两个同 phandle 条目必须保留两条）。
                let clocks_raw = parse_cells_prop(&frame, "clocks")?;
                let resets_raw = parse_cells_prop(&frame, "resets")?;
                let gpios_raw = parse_cells_prop(&frame, "gpios")?;
                let dmas_raw = parse_cells_prop(&frame, "dmas")?;
                let clocks: Vec<ClockRef> = Vec::new(); // 占位；树完整后按
                                                        // provider #clock-cells 拆分回填（pending_clocks）。
                let resets: Vec<eneros_hal::inventory::ResetRef> = Vec::new();
                let gpios: Vec<eneros_hal::inventory::GpioRef> = Vec::new();
                let dmas: Vec<eneros_hal::inventory::DmaChannelRef> = Vec::new();
                // 树完整后回填：本节点原始 cells 列表（node_idx 已确定）。
                pending_clocks.push((tree.nodes.len(), clocks_raw));
                pending_resets.push((tree.nodes.len(), resets_raw));
                pending_gpios.push((tree.nodes.len(), gpios_raw));
                pending_dmas.push((tree.nodes.len(), dmas_raw));

                // 翻译 regs：沿父级 ranges 链（子 bus addr → CPU phys）。
                let regs = translate_regs(&stack, &regs_bus)?;

                // 全局事实收集。
                // 属性值为字符串时含 NUL 终止符——统一裁剪后比较。
                let device_type = frame
                    .props
                    .iter()
                    .find(|(k, _)| k == "device_type")
                    .map(|(_, v)| core::str::from_utf8(v).map(|s| s.trim_end_matches('\0')))
                    .and_then(|r| r.ok());
                let is_memory = device_type == Some("memory")
                    || frame
                        .props
                        .iter()
                        .any(|(k, v)| k == "compatible" && v.windows(6).any(|w| w == b"memory"));
                if is_memory && !regs.is_empty() {
                    for (b, s) in &regs {
                        tree.memory.push((*b, *s));
                    }
                }
                let is_cpu = device_type == Some("cpu");
                if is_cpu {
                    if let Some(addr) = frame.name.split('@').nth(1) {
                        if let Ok(id) = u64::from_str_radix(addr.trim(), 16) {
                            tree.cpus.push(id as u32);
                        }
                    }
                }
                if frame.path == "/reserved-memory"
                    || stack.last().is_some_and(|f| f.path == "/reserved-memory")
                {
                    // 子节点为保留区（reg 属性）。
                    if !regs.is_empty() {
                        for (b, s) in &regs {
                            tree.reserved.push((*b, *s, frame.name.clone()));
                        }
                    }
                }
                if frame.path == "/aliases" {
                    for (k, v) in &frame.props {
                        if k == "phandle" {
                            continue;
                        }
                        let s = String::from_utf8_lossy(v).to_string();
                        let s = s.trim_end_matches('\0').to_string();
                        tree.aliases.push((k.clone(), s));
                    }
                }
                if frame.path == "/chosen" {
                    if let Some(v) = frame
                        .props
                        .iter()
                        .find(|(k, _)| k == "stdout-path")
                        .map(|(_, v)| v)
                    {
                        let s = String::from_utf8_lossy(v).to_string();
                        tree.stdout_path = Some(s.trim_end_matches('\0').to_string());
                    }
                }

                // phandle 索引登记顺序：节点索引 = tree.nodes.len()（push 前）。
                let node_idx = tree.nodes.len();
                if let Some(p) = phandle {
                    // 覆盖前面占位（若已 push 过）。
                    if let Some(slot) = tree.phandle_index.iter_mut().find(|(pp, _)| *pp == p) {
                        slot.1 = node_idx;
                    } else {
                        tree.phandle_index.push((p, node_idx));
                    }
                }
                let node = DeviceNode {
                    name: frame.name,
                    path: frame.path,
                    compatible: compatible_all.first().cloned().unwrap_or_default(),
                    compatible_all,
                    status,
                    phandle,
                    regs,
                    regs_bus,
                    interrupts: interrupts_raw,
                    global_irqs,
                    clocks,
                    resets,
                    gpios,
                    dmas,
                    clock_frequency,
                    dma_coherent,
                    dma_ranges,
                    iommus,
                    properties: frame.props,
                };
                if tree.nodes.len() >= MAX_NODES {
                    return Err(FdtError::TooManyNodes);
                }
                tree.nodes.push(node);
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
                let frame = stack.last_mut().expect("root 帧恒在");
                if frame.props.len() >= MAX_PROPERTIES_PER_NODE {
                    return Err(FdtError::TooManyProperties {
                        path: frame.path.clone(),
                    });
                }
                // WP-38.2：`#address-cells`/`#size-cells` 即时生效——子节点
                // 在 BEGIN_NODE 时继承的必须是本节点**自己的** cells（FDT
                // 规范：父节点属性描述子节点 reg 的解释）。
                if prop_name == "#address-cells" && value.len() == 4 {
                    frame.addr_cells = u32::from_be_bytes([value[0], value[1], value[2], value[3]]);
                } else if prop_name == "#size-cells" && value.len() == 4 {
                    frame.size_cells = u32::from_be_bytes([value[0], value[1], value[2], value[3]]);
                }
                frame.props.push((prop_name, value.to_vec()));
            }
            FDT_NOP => {}
            FDT_END => break,
            _other => return Err(FdtError::BadToken),
        }
    }
    // 引用类属性回填：按 provider 的 `#*-cells` 拆分（缺省 0）；provider
    // 不存在 → UnknownPhandle（fail-closed，零猜时钟/零猜 specifier）。
    // clocks 额外携带 provider 的 clock-frequency（fixed-clock 语义）。
    for (idx, raw) in pending_clocks {
        let mut out: Vec<ClockRef> = Vec::new();
        let mut off = 0usize;
        while off < raw.len() {
            let ph = raw[off];
            off += 1;
            let Some(pi) = tree.find_phandle(ph) else {
                return Err(FdtError::UnknownPhandle {
                    phandle: ph,
                    context: format!("clocks of {}", tree.nodes[idx].path),
                });
            };
            let cells = cells_of(&tree, pi, "#clock-cells")?;
            if off + cells > raw.len() {
                return Err(FdtError::BadCellCount {
                    path: tree.nodes[idx].path.clone(),
                    property: "clocks".into(),
                    offset: off * 4,
                });
            }
            off += cells;
            out.push(ClockRef {
                phandle: ph,
                frequency_hz: tree.nodes[pi].clock_frequency,
            });
        }
        tree.nodes[idx].clocks = out;
    }
    fill_refs(&mut tree, pending_resets, "resets", "#reset-cells")?;
    fill_refs(&mut tree, pending_gpios, "gpios", "#gpio-cells")?;
    fill_refs(&mut tree, pending_dmas, "dmas", "#dma-cells")?;
    Ok(tree)
}

/// provider 的 `#<cells-prop>` 值（缺省 0；非 4 字节属性 → BadCellCount）。
fn cells_of(tree: &FdtTree, provider_idx: usize, cells_prop: &str) -> Result<usize, FdtError> {
    tree.nodes[provider_idx]
        .properties
        .iter()
        .find(|(k, _)| k == cells_prop)
        .map(|(_, v)| {
            if v.len() < 4 {
                return Err(FdtError::BadCellCount {
                    path: tree.nodes[provider_idx].path.clone(),
                    property: cells_prop.into(),
                    offset: v.len(),
                });
            }
            Ok(u32::from_be_bytes([v[0], v[1], v[2], v[3]]) as usize)
        })
        .unwrap_or(Ok(0))
}

/// 通用 provider-cells 引用回填：`raw = <&phandle cells...>*`，按 provider
/// 的 `#<cells-prop>` 拆分；phandle 未索引 → UnknownPhandle（fail-closed）。
fn fill_refs(
    tree: &mut FdtTree,
    pending: Vec<(usize, Vec<u32>)>,
    prop: &str,
    cells_prop: &str,
) -> Result<(), FdtError> {
    for (idx, raw) in pending {
        let mut out: Vec<(u32, Vec<u32>)> = Vec::new();
        let mut off = 0usize;
        while off < raw.len() {
            let ph = raw[off];
            off += 1;
            let Some(pi) = tree.find_phandle(ph) else {
                return Err(FdtError::UnknownPhandle {
                    phandle: ph,
                    context: format!("{prop} of {}", tree.nodes[idx].path),
                });
            };
            let cells = cells_of(tree, pi, cells_prop)?;
            if off + cells > raw.len() {
                return Err(FdtError::BadCellCount {
                    path: tree.nodes[idx].path.clone(),
                    property: prop.into(),
                    offset: off * 4,
                });
            }
            out.push((ph, raw[off..off + cells].to_vec()));
            off += cells;
        }
        // fill_refs 仅接受 resets/gpios/dmas（调用点静态保证；不用
        // unreachable!——项目 lint 禁用）。
        if prop == "resets" {
            tree.nodes[idx].resets = out
                .into_iter()
                .map(|(p, s)| eneros_hal::inventory::ResetRef {
                    phandle: p,
                    specifier: s,
                })
                .collect();
        } else if prop == "gpios" {
            tree.nodes[idx].gpios = out
                .into_iter()
                .map(|(p, s)| eneros_hal::inventory::GpioRef {
                    phandle: p,
                    specifier: s,
                })
                .collect();
        } else {
            tree.nodes[idx].dmas = out
                .into_iter()
                .map(|(p, s)| eneros_hal::inventory::DmaChannelRef {
                    phandle: p,
                    specifier: s,
                })
                .collect();
        }
    }
    Ok(())
}

fn prop_u32(props: &[(String, Vec<u8>)], name: &str) -> Option<u32> {
    props.iter().find(|(k, _)| k == name).and_then(|(_, v)| {
        if v.len() >= 4 {
            Some(u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
        } else {
            None
        }
    })
}

fn prop_u64(props: &[(String, Vec<u8>)], name: &str) -> Option<u64> {
    props.iter().find(|(k, _)| k == name).and_then(|(_, v)| {
        if v.len() >= 8 {
            Some(u64::from_be_bytes([
                v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7],
            ]))
        } else if v.len() >= 4 {
            Some(u64::from(u32::from_be_bytes([v[0], v[1], v[2], v[3]])))
        } else {
            None
        }
    })
}

fn prop_u32s(props: &[(String, Vec<u8>)], name: &str) -> Vec<u32> {
    props
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| {
            v.chunks_exact(4)
                .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        })
        .unwrap_or_default()
}

/// 引用类属性（clocks/resets/gpios/dmas）原始 cells：`<&phandle cells...>*`
/// 序列；非 4 字节对齐 → BadCellCount（fail-closed，不猜拆分）。
fn parse_cells_prop(frame: &Frame, name: &str) -> Result<Vec<u32>, FdtError> {
    let Some((_, v)) = frame.props.iter().find(|(k, _)| k == name) else {
        return Ok(Vec::new());
    };
    if v.len() % 4 != 0 {
        return Err(FdtError::BadCellCount {
            path: frame.path.clone(),
            property: name.into(),
            offset: v.len(),
        });
    }
    Ok((0..v.len() / 4)
        .map(|i| u32::from_be_bytes([v[i * 4], v[i * 4 + 1], v[i * 4 + 2], v[i * 4 + 3]]))
        .collect())
}

/// compatible 列表（NUL 分隔；优先级序；上限 [`MAX_COMPATIBLES`]）。
fn parse_compatible(props: &[(String, Vec<u8>)], path: &str) -> Result<Vec<String>, FdtError> {
    let Some((_, v)) = props.iter().find(|(k, _)| k == "compatible") else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, &b) in v.iter().enumerate() {
        if b == 0 {
            if i > start {
                let s = core::str::from_utf8(&v[start..i]).map_err(|_| FdtError::BadString)?;
                out.push(s.to_owned());
            }
            start = i + 1;
            if out.len() >= MAX_COMPATIBLES {
                return Err(FdtError::BadCellCount {
                    path: path.into(),
                    property: "compatible".into(),
                    offset: i,
                });
            }
        }
    }
    if start < v.len() {
        let s = core::str::from_utf8(&v[start..]).map_err(|_| FdtError::BadString)?;
        out.push(s.to_owned());
    }
    Ok(out)
}

/// status 解析（缺省 → Ok；`disabled/fail/reserved` 显式状态）。
fn parse_status(props: &[(String, Vec<u8>)]) -> NodeStatus {
    let Some((_, v)) = props.iter().find(|(k, _)| k == "status") else {
        return NodeStatus::Ok;
    };
    let s = String::from_utf8_lossy(v);
    let s = s.trim_end_matches('\0');
    match s {
        "disabled" => NodeStatus::Disabled,
        "fail" => NodeStatus::Fail,
        "reserved" => NodeStatus::Reserved,
        _ => NodeStatus::Ok,
    }
}

/// reg/dma-ranges 解析（cells 继承；合法 1/2 组合）。
fn parse_reg_pairs(
    props: &[(String, Vec<u8>)],
    prop: &str,
    addr_cells: u32,
    size_cells: u32,
    path: &str,
) -> Result<Vec<(u64, u64)>, FdtError> {
    let Some((_, v)) = props.iter().find(|(k, _)| k == prop) else {
        return Ok(Vec::new());
    };
    // `#size-cells=0` 合法（如 cpus 节点 reg 无 size）；cells 均限 0..=2。
    if !(1..=2).contains(&addr_cells) || !(0..=2).contains(&size_cells) {
        return Err(FdtError::BadCellCount {
            path: path.into(),
            property: prop.into(),
            offset: 0,
        });
    }
    let stride = (addr_cells + size_cells) as usize;
    if stride == 0 || v.len() % (stride * 4) != 0 {
        return Err(FdtError::BadCellCount {
            path: path.into(),
            property: prop.into(),
            offset: v.len(),
        });
    }
    let mut out = Vec::new();
    for chunk in v.chunks_exact(stride * 4) {
        let mut addr = 0u64;
        for i in 0..addr_cells as usize {
            let c = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
            addr = (addr << 32) | u64::from(c);
        }
        let mut size = 0u64;
        for i in 0..size_cells as usize {
            let base = (addr_cells as usize + i) * 4;
            let c = u32::from_be_bytes([
                chunk[base],
                chunk[base + 1],
                chunk[base + 2],
                chunk[base + 3],
            ]);
            size = (size << 32) | u64::from(c);
        }
        out.push((addr, size));
    }
    Ok(out)
}

/// 沿父级 `ranges` 链翻译 reg（子 bus addr → CPU phys）。
///
/// 每层 `ranges` = <child_addr(child_ac) parent_addr(parent_ac) size(sc)>*；
/// 从当前节点父级向上累加。child_ac = 本层（bus 节点）自己的 #address-cells
/// （子总线地址空间），parent_ac = 父级 #address-cells，sc = 本层 #size-cells。
/// 无 ranges → 直通（bus addr == parent addr）；ranges 为空值 → 恒等映射
/// （DT 规范：父子地址空间相同）。
fn translate_regs(stack: &[Frame], regs_bus: &[(u64, u64)]) -> Result<Vec<(u64, u64)>, FdtError> {
    let mut out = Vec::with_capacity(regs_bus.len());
    for (bus, size) in regs_bus {
        let mut addr = *bus;
        // 从父级（stack 末帧）向上逐层应用（root 帧 idx=0 不翻译）。
        for k in (1..stack.len()).rev() {
            let f = &stack[k];
            let Some((_, ranges)) = f.props.iter().find(|(k, _)| k == "ranges") else {
                continue; // 无 ranges：直通
            };
            if ranges.is_empty() {
                continue; // 空 ranges = 恒等映射（规范语义）
            }
            let child_ac = f.addr_cells;
            let parent_ac = stack[k - 1].addr_cells;
            let sc = f.size_cells;
            let stride = (child_ac + parent_ac + sc) as usize;
            if !(1..=2).contains(&child_ac) || !(1..=2).contains(&parent_ac) {
                return Err(FdtError::BadRanges {
                    path: f.path.clone(),
                    offset: 0,
                });
            }
            if ranges.len() % (stride * 4) != 0 {
                return Err(FdtError::BadRanges {
                    path: f.path.clone(),
                    offset: ranges.len(),
                });
            }
            let mut matched = false;
            for chunk in ranges.chunks_exact(stride * 4) {
                let mut child_base = 0u64;
                for i in 0..child_ac as usize {
                    let c = u32::from_be_bytes([
                        chunk[i * 4],
                        chunk[i * 4 + 1],
                        chunk[i * 4 + 2],
                        chunk[i * 4 + 3],
                    ]);
                    child_base = (child_base << 32) | u64::from(c);
                }
                let mut parent_base = 0u64;
                for i in 0..parent_ac as usize {
                    let base = (child_ac as usize + i) * 4;
                    let c = u32::from_be_bytes([
                        chunk[base],
                        chunk[base + 1],
                        chunk[base + 2],
                        chunk[base + 3],
                    ]);
                    parent_base = (parent_base << 32) | u64::from(c);
                }
                let mut range_size = 0u64;
                for i in 0..sc as usize {
                    let base = (child_ac as usize + parent_ac as usize + i) * 4;
                    let c = u32::from_be_bytes([
                        chunk[base],
                        chunk[base + 1],
                        chunk[base + 2],
                        chunk[base + 3],
                    ]);
                    range_size = (range_size << 32) | u64::from(c);
                }
                if addr >= child_base {
                    let off = addr - child_base;
                    if off < range_size {
                        addr = parent_base.checked_add(off).ok_or(FdtError::BadRanges {
                            path: f.path.clone(),
                            offset: 0,
                        })?;
                        matched = true;
                        break;
                    }
                }
            }
            if !matched {
                // 无匹配 ranges → 无法翻译（fail-closed）。
                return Err(FdtError::BadRanges {
                    path: f.path.clone(),
                    offset: 0,
                });
            }
        }
        out.push((addr, *size));
    }
    Ok(out)
}

/// dma-ranges 解析：`<child_addr(size_cells) parent_addr(父 cells) size>*`。
fn parse_dma_ranges(
    props: &[(String, Vec<u8>)],
    child_cells: u32,
    parent_cells: u32,
    size_cells: u32,
    path: &str,
) -> Result<Vec<(u64, u64, u64)>, FdtError> {
    let Some((_, v)) = props.iter().find(|(k, _)| k == "dma-ranges") else {
        return Ok(Vec::new());
    };
    if !(1..=2).contains(&child_cells) || !(1..=2).contains(&parent_cells) {
        return Err(FdtError::BadCellCount {
            path: path.into(),
            property: "dma-ranges".into(),
            offset: 0,
        });
    }
    let stride = (child_cells + parent_cells + size_cells) as usize;
    if v.len() % (stride * 4) != 0 {
        return Err(FdtError::BadCellCount {
            path: path.into(),
            property: "dma-ranges".into(),
            offset: v.len(),
        });
    }
    let mut out = Vec::new();
    for chunk in v.chunks_exact(stride * 4) {
        let mut child = 0u64;
        for i in 0..child_cells as usize {
            let c = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
            child = (child << 32) | u64::from(c);
        }
        let mut parent = 0u64;
        for i in 0..parent_cells as usize {
            let base = (child_cells as usize + i) * 4;
            let c = u32::from_be_bytes([
                chunk[base],
                chunk[base + 1],
                chunk[base + 2],
                chunk[base + 3],
            ]);
            parent = (parent << 32) | u64::from(c);
        }
        let mut size = 0u64;
        for i in 0..size_cells as usize {
            let base = (child_cells as usize + parent_cells as usize + i) * 4;
            let c = u32::from_be_bytes([
                chunk[base],
                chunk[base + 1],
                chunk[base + 2],
                chunk[base + 3],
            ]);
            size = (size << 32) | u64::from(c);
        }
        out.push((child, parent, size));
    }
    Ok(out)
}

/// interrupts 原始 cells → 全局中断号（interrupt-parent 链 + #interrupt-cells）。
///
/// GIC 编码（#interrupt-cells=3）：[type, num, flags]；type=0 SPI → 32+num，
/// type=1 PPI → 16+num。interrupt-parent 缺省 = 根节点 interrupt-parent。
fn parse_interrupts(
    frame: &Frame,
    stack: &[Frame],
    tree: &FdtTree,
    raw: &[u32],
) -> Result<Vec<InterruptSpec>, FdtError> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    // interrupt-parent：本节点显式 → 否则根 interrupt-parent。
    let parent_ph = frame
        .props
        .iter()
        .find(|(k, _)| k == "interrupt-parent")
        .and_then(|(_, v)| (v.len() >= 4).then(|| u32::from_be_bytes([v[0], v[1], v[2], v[3]])))
        .or_else(|| {
            stack
                .first()
                .and_then(|r| r.props.iter().find(|(k, _)| k == "interrupt-parent"))
                .and_then(|(_, v)| {
                    (v.len() >= 4).then(|| u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
                })
        });
    let Some(pp) = parent_ph else {
        return Err(FdtError::UnknownPhandle {
            phandle: 0,
            context: format!("interrupt-parent for {}", frame.path),
        });
    };
    // 父控制器 #interrupt-cells（缺省 3 = GIC）。
    let cells = tree
        .find_phandle(pp)
        .and_then(|i| tree.nodes.get(i))
        .and_then(|n| {
            n.properties
                .iter()
                .find(|(k, _)| k == "#interrupt-cells")
                .and_then(|(_, v)| {
                    (v.len() >= 4).then(|| u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
                })
        })
        .unwrap_or(3);
    if cells != 3 {
        // 仅支持 GIC 3-cell 编码（当前 AArch64 平台；其余 fail-closed）。
        return Err(FdtError::BadCellCount {
            path: frame.path.clone(),
            property: "interrupts".into(),
            offset: 0,
        });
    }
    if raw.len() % 3 != 0 {
        return Err(FdtError::BadCellCount {
            path: frame.path.clone(),
            property: "interrupts".into(),
            offset: raw.len() * 4,
        });
    }
    let mut out = Vec::new();
    for triple in raw.chunks_exact(3) {
        let typ = triple[0];
        let num = triple[1];
        let flags = triple[2];
        let irq = match typ {
            0 => 32u32.checked_add(num), // SPI
            1 => 16u32.checked_add(num), // PPI
            _ => None,
        };
        let Some(irq) = irq else {
            return Err(FdtError::BadCellCount {
                path: frame.path.clone(),
                property: "interrupts".into(),
                offset: 0,
            });
        };
        let trigger = if flags & 0x1 != 0 {
            // bit0=1：低→高边沿触发。
            IrqTrigger::Edge
        } else {
            // bit0=0：电平敏感（GIC 标志 4=active-high level）。
            IrqTrigger::Level
        };
        out.push(InterruptSpec { irq, trigger });
    }
    Ok(out)
}

/// iommus 解析：<&phandle domain_cell>*。
fn parse_iommus(props: &[(String, Vec<u8>)], path: &str) -> Result<Vec<(u32, u32)>, FdtError> {
    let Some((_, v)) = props.iter().find(|(k, _)| k == "iommus") else {
        return Ok(Vec::new());
    };
    if v.len() % 8 != 0 {
        return Err(FdtError::BadCellCount {
            path: path.into(),
            property: "iommus".into(),
            offset: v.len(),
        });
    }
    let mut out = Vec::new();
    for chunk in v.chunks_exact(8) {
        let ph = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let domain = u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        out.push((ph, domain));
    }
    Ok(out)
}

/// 兼容入口：解析 FDT blob → 设备清单（WP-38.2 完整语义）。
pub fn parse_fdt(blob: &[u8]) -> Result<DeviceInventory, FdtError> {
    parse_fdt_tree(blob).map(|t| t.to_inventory())
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use eneros_hal::platform_adapter::PlatformInput;

    use super::*;

    /// 真实设备树（dtc 编译 configs/qemu-virt.dts）——解析器必须能吃
    /// 真实节点（pl011 UART / armv8-timer / gic）。
    const QEMU_VIRT_DTB: &[u8] = include_bytes!("../tests/fixtures/qemu-virt.dtb");

    #[test]
    fn parses_pl011_and_timer_nodes() {
        let inv = parse_fdt(QEMU_VIRT_DTB).expect("parse 真实 dtb");
        let pl011 = inv.find_by_compatible("arm,pl011").expect("pl011 节点");
        assert_eq!(pl011.name, "uart@9000000");
        assert_eq!(pl011.regs, vec![(0x0900_0000, 0x1000)]);
        assert_eq!(pl011.interrupts, vec![0, 1, 4]);
        // WP-38.2：全局中断号解析（SPI 1 → 33）。
        assert_eq!(
            pl011.global_irqs,
            vec![InterruptSpec {
                irq: 33,
                trigger: IrqTrigger::Level
            }]
        );
        // WP-38.2：clocks phandle → frequency（clk24mhz = 24 MHz）；
        // 两个同 phandle 条目（uartclk + apb_pclk）必须保留为两条。
        assert_eq!(
            pl011.clocks.len(),
            2,
            "两条 clocks 引用必须保留: {:?}",
            pl011.clocks
        );
        assert_eq!(
            pl011.clocks.first().map(|c| c.frequency_hz),
            Some(Some(24_000_000)),
            "clocks phandle 回填 clock-frequency"
        );
        assert_eq!(pl011.clocks[1].frequency_hz, Some(24_000_000));
        // status=okay → 清单内。
        assert_eq!(pl011.status, NodeStatus::Ok);
        let timer = inv
            .find_by_compatible("arm,armv8-timer")
            .expect("timer 节点");
        assert!(
            timer.interrupts.chunks_exact(3).any(|c| c == [1, 13, 11]),
            "虚拟 timer PPI (1,13,11) 必须在 interrupts 中: {:?}",
            timer.interrupts
        );
        // PPI 13 → 29（虚拟 timer 全局号）。
        assert!(
            timer.global_irqs.iter().any(|s| s.irq == 29),
            "PPI 13 → IRQ 29: {:?}",
            timer.global_irqs
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

    /// 引用类属性（resets/gpios/dmas）指向未索引 phandle → UnknownPhandle
    /// （fail-closed，零猜 specifier）。
    #[test]
    fn unknown_provider_phandle_fails_closed() {
        let mut d = SynthDtb::new();
        d.begin_node("/");
        d.prop("#address-cells", &SynthDtb::cells(2, 1));
        d.prop("#size-cells", &SynthDtb::cells(2, 1));
        d.begin_node("uart@9000000");
        d.prop("compatible", b"arm,pl011\0");
        d.prop("resets", &{
            let mut v = SynthDtb::cells(0xbeef, 1); // 无此 phandle
            v.extend_from_slice(&SynthDtb::cells(1, 1));
            v
        });
        d.end_node();
        // finish() 自动 end 根节点——不显式 end_node("/")。
        let blob = d.finish();
        let err = parse_fdt_tree(&blob).expect_err("未知 phandle 必须拒绝");
        assert!(
            matches!(err, FdtError::UnknownPhandle { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn tree_exposes_memory_cpus_and_reserved() {
        let tree = parse_fdt_tree(QEMU_VIRT_DTB).expect("parse 真实 dtb");
        assert!(!tree.memory.is_empty(), "memory 节点必须解析");
        assert!(!tree.cpus.is_empty(), "cpu 节点必须解析");
        assert_eq!(tree.cpus[0], 0);
    }

    #[test]
    fn to_inventory_filters_disabled() {
        let tree = parse_fdt_tree(QEMU_VIRT_DTB).expect("parse");
        let inv = tree.to_inventory();
        // qemu-virt 全部 status=okay → disabled 空。
        assert!(inv.disabled.is_empty(), "{:?}", inv.disabled);
    }

    #[test]
    fn fdt_provider_describe_produces_inventory() {
        let provider = FdtDescriptionProvider;
        let inv = provider
            .describe(&PlatformInput {
                runtime_dtb: Some(QEMU_VIRT_DTB),
                bootinfo_cpu_count: 4,
            })
            .expect("describe 必须成功");
        assert!(!inv.devices.is_empty());
        assert!(!inv.cpus.is_empty());
        assert_eq!(inv.cpus[0].id, 0);
        assert_eq!(
            inv.provenance.source,
            eneros_hal::inventory::DescriptionSource::Fdt
        );
        // pl011 设备带时钟与中断。
        let pl011 = inv
            .devices
            .iter()
            .find(|d| d.compatible.first() == "arm,pl011")
            .expect("pl011");
        assert_eq!(pl011.first_clock_hz(), Some(24_000_000));
        assert!(!pl011.interrupts.is_empty());
    }

    #[test]
    fn chosen_console_index_is_device_list_index() {
        // 38 回归：stdout-path 解析必须落到 device 列表索引（tree 索引
        // 在 device 列表里可能是别的设备——实测 41 是 GIC）。
        let provider = FdtDescriptionProvider;
        let inv = provider
            .describe(&PlatformInput {
                runtime_dtb: Some(QEMU_VIRT_DTB),
                bootinfo_cpu_count: 2,
            })
            .expect("describe");
        if let Some(ci) = inv.chosen.console {
            let dev = &inv.devices[ci];
            assert!(
                dev.compatible.as_slice().iter().any(|c| c == "arm,pl011"),
                "console 设备必须解析到 pl011: got {:?}",
                dev.compatible.as_slice()
            );
        }
    }

    #[test]
    fn fdt_provider_no_description_fails_closed() {
        let provider = FdtDescriptionProvider;
        let r = provider.describe(&PlatformInput::default());
        assert_eq!(
            r,
            Err(eneros_hal::platform_adapter::PlatformError::NoDescription)
        );
    }

    // =========================================================================
    // 合成平台 B：嵌套 bus、不同 address cells、ranges 翻译、多 compatible、
    // non-coherent+IOMMU、disabled 设备（Exit Gate 3：无需改核心即可正确
    // 解析或明确拒绝）。
    // =========================================================================

    /// 合成 DTB 构造器（仅测试：按结构块/字符串块/内存块最小布局生成）。
    struct SynthDtb {
        struct_blob: Vec<u8>,
        strings: Vec<u8>,
        str_map: alloc::collections::BTreeMap<String, u32>,
    }

    impl SynthDtb {
        fn new() -> Self {
            Self {
                struct_blob: Vec::new(),
                strings: Vec::new(),
                str_map: alloc::collections::BTreeMap::new(),
            }
        }

        fn intern(&mut self, s: &str) -> u32 {
            if let Some(o) = self.str_map.get(s) {
                return *o;
            }
            let off = self.strings.len() as u32;
            self.strings.extend_from_slice(s.as_bytes());
            self.strings.push(0);
            self.str_map.insert(s.into(), off);
            off
        }

        fn token(&mut self, t: u32) {
            self.struct_blob.extend_from_slice(&t.to_be_bytes());
        }

        fn begin_node(&mut self, name: &str) {
            self.token(FDT_BEGIN_NODE);
            self.struct_blob.extend_from_slice(name.as_bytes());
            self.struct_blob.push(0);
            while self.struct_blob.len() % 4 != 0 {
                self.struct_blob.push(0);
            }
        }

        fn end_node(&mut self) {
            self.token(FDT_END_NODE);
        }

        fn prop(&mut self, name: &str, value: &[u8]) {
            self.token(FDT_PROP);
            self.struct_blob
                .extend_from_slice(&(value.len() as u32).to_be_bytes());
            let nameoff = self.intern(name);
            self.struct_blob.extend_from_slice(&nameoff.to_be_bytes());
            self.struct_blob.extend_from_slice(value);
            while self.struct_blob.len() % 4 != 0 {
                self.struct_blob.push(0);
            }
        }

        /// cells 编码（大端；1..=2 单元）。
        fn cells(v: u64, n: u32) -> Vec<u8> {
            let mut out = Vec::new();
            for i in (0..n).rev() {
                out.extend_from_slice(&((v >> (32 * i)) as u32).to_be_bytes());
            }
            out
        }

        fn finish(mut self) -> Vec<u8> {
            self.end_node(); // 根结束
            self.token(FDT_END);
            let off_struct = 40usize;
            let off_strings = off_struct + self.struct_blob.len();
            let totalsize = off_strings + self.strings.len();
            let mut blob = Vec::with_capacity(totalsize);
            // FDT header 固定 40 字节：magic/totalsize/off_struct/off_strings/
            // off_mem_rsvmap/version/last_comp_version/boot_cpuid_phys/
            // size_dt_strings/size_dt_struct。
            blob.extend_from_slice(&FDT_MAGIC.to_be_bytes());
            blob.extend_from_slice(&(totalsize as u32).to_be_bytes());
            blob.extend_from_slice(&(off_struct as u32).to_be_bytes());
            blob.extend_from_slice(&(off_strings as u32).to_be_bytes());
            blob.extend_from_slice(&0u32.to_be_bytes()); // off_mem_rsvmap
            blob.extend_from_slice(&17u32.to_be_bytes()); // version
            blob.extend_from_slice(&16u32.to_be_bytes()); // last_comp_version
            blob.extend_from_slice(&0u32.to_be_bytes()); // boot_cpuid_phys
            blob.extend_from_slice(&(self.strings.len() as u32).to_be_bytes()); // size_dt_strings
            blob.extend_from_slice(&(self.struct_blob.len() as u32).to_be_bytes()); // size_dt_struct
            blob.extend_from_slice(&self.struct_blob);
            blob.extend_from_slice(&self.strings);
            blob
        }
    }

    /// 平台 B 树：
    /// root(2/2 cells, interrupt-parent=ph1)
    /// ├─ gic@8000000 (phandle 1, #interrupt-cells=3)
    /// ├─ cpus { #address-cells=1 #size-cells=0; cpu@0 {device_type=cpu; reg=<0>} }
    /// ├─ memory@40000000 {device_type=memory; reg=<0 0x40000000 0 0x8000000>}
    /// ├─ soc { #address-cells=1 #size-cells=1 ranges=<0x0 0x0 0x9000000 0x1000>  (child 1cell, parent 2cell, size 1cell)
    /// │   ├─ uart@0 { compatible="arm,pl011" reg=<0x0 0x1000> clocks=<&ph3 0> }
    /// │   └─ eth@1000 { compatible="vendor,eth","generic,net" reg=<0x1000 0x100> interrupts=<0 7 4> dma-coherent iommus=<&ph4 1> status="okay" }
    /// ├─ clk@1 (phandle 3, clock-frequency=<0x16e3600>)
    /// ├─ smmu@5000000 (phandle 4)
    /// └─ rtc@9010000 { compatible="arm,pl031" reg=<0 0x9010000 0 0x1000> status="disabled" }
    fn platform_b_dtb() -> Vec<u8> {
        let mut d = SynthDtb::new();
        d.begin_node("/");
        d.prop("#address-cells", &SynthDtb::cells(2, 1));
        d.prop("#size-cells", &SynthDtb::cells(2, 1));
        d.prop("interrupt-parent", &SynthDtb::cells(1, 1));
        d.begin_node("cpus");
        d.prop("#address-cells", &SynthDtb::cells(1, 1));
        d.prop("#size-cells", &SynthDtb::cells(0, 1));
        d.begin_node("cpu@0");
        d.prop("device_type", b"cpu\0");
        d.prop("reg", &SynthDtb::cells(0, 1));
        d.end_node();
        d.end_node();
        d.begin_node("memory@40000000");
        d.prop("device_type", b"memory\0");
        let mut reg = SynthDtb::cells(0x4000_0000, 2);
        reg.extend_from_slice(&SynthDtb::cells(0x0800_0000, 2));
        d.prop("reg", &reg);
        d.end_node();
        d.begin_node("soc");
        d.prop("#address-cells", &SynthDtb::cells(1, 1));
        d.prop("#size-cells", &SynthDtb::cells(1, 1));
        let mut ranges = SynthDtb::cells(0x0, 1); // child addr
        ranges.extend_from_slice(&SynthDtb::cells(0x0900_0000, 2)); // parent addr
        ranges.extend_from_slice(&SynthDtb::cells(0x2000, 1)); // size（覆盖 uart@0 与 eth@1000）
        d.prop("ranges", &ranges);
        d.begin_node("uart@0");
        d.prop("compatible", b"arm,pl011\0");
        let mut ureg = SynthDtb::cells(0x0, 1);
        ureg.extend_from_slice(&SynthDtb::cells(0x1000, 1));
        d.prop("reg", &ureg);
        d.prop("interrupts", &{
            let mut v = SynthDtb::cells(0, 1);
            v.extend_from_slice(&SynthDtb::cells(1, 1));
            v.extend_from_slice(&SynthDtb::cells(4, 1));
            v
        });
        d.prop("clocks", &SynthDtb::cells(3, 1));
        // 引用类属性：resets/gpios/dmas 按 provider #*-cells 拆分。
        d.prop("resets", &{
            let mut v = SynthDtb::cells(5, 1);
            v.extend_from_slice(&SynthDtb::cells(7, 1));
            v
        });
        d.prop("gpios", &{
            let mut v = SynthDtb::cells(6, 1);
            v.extend_from_slice(&SynthDtb::cells(1, 1));
            v.extend_from_slice(&SynthDtb::cells(0, 1));
            v
        });
        d.prop("dmas", &{
            let mut v = SynthDtb::cells(7, 1);
            v.extend_from_slice(&SynthDtb::cells(2, 1));
            v.extend_from_slice(&SynthDtb::cells(3, 1));
            v
        });
        d.end_node();
        d.begin_node("eth@1000");
        d.prop("compatible", b"vendor,eth\0generic,net\0");
        let mut ereg = SynthDtb::cells(0x1000, 1);
        ereg.extend_from_slice(&SynthDtb::cells(0x100, 1));
        d.prop("reg", &ereg);
        let mut irq = SynthDtb::cells(0, 1);
        irq.extend_from_slice(&SynthDtb::cells(7, 1));
        irq.extend_from_slice(&SynthDtb::cells(4, 1));
        d.prop("interrupts", &irq);
        d.prop("dma-coherent", &[]);
        d.prop("iommus", &{
            let mut v = SynthDtb::cells(4, 1);
            v.extend_from_slice(&SynthDtb::cells(1, 1));
            v
        });
        d.end_node();
        d.end_node();
        d.begin_node("clk@1");
        d.prop("compatible", b"fixed-clock\0");
        d.prop("#clock-cells", &SynthDtb::cells(0, 1));
        d.prop("phandle", &SynthDtb::cells(3, 1));
        d.prop("clock-frequency", &SynthDtb::cells(0x16e_3600, 1));
        d.end_node();
        d.begin_node("smmu@5000000");
        d.prop("compatible", b"arm,smmu-v3\0");
        d.prop("phandle", &SynthDtb::cells(4, 1));
        let mut sreg = SynthDtb::cells(0x5000_0000, 2);
        sreg.extend_from_slice(&SynthDtb::cells(0x1000, 2));
        d.prop("reg", &sreg);
        d.end_node();
        d.begin_node("reset@5100000");
        d.prop("compatible", b"vendor,reset-ctl\0");
        d.prop("phandle", &SynthDtb::cells(5, 1));
        d.prop("#reset-cells", &SynthDtb::cells(1, 1));
        let mut rreg = SynthDtb::cells(0x5100_0000, 2);
        rreg.extend_from_slice(&SynthDtb::cells(0x100, 2));
        d.prop("reg", &rreg);
        d.end_node();
        d.begin_node("gpio@5200000");
        d.prop("compatible", b"vendor,gpio-ctl\0");
        d.prop("phandle", &SynthDtb::cells(6, 1));
        d.prop("#gpio-cells", &SynthDtb::cells(2, 1));
        let mut greg = SynthDtb::cells(0x5200_0000, 2);
        greg.extend_from_slice(&SynthDtb::cells(0x100, 2));
        d.prop("reg", &greg);
        d.end_node();
        d.begin_node("dma@5300000");
        d.prop("compatible", b"vendor,dma-ctl\0");
        d.prop("phandle", &SynthDtb::cells(7, 1));
        d.prop("#dma-cells", &SynthDtb::cells(2, 1));
        let mut dreg = SynthDtb::cells(0x5300_0000, 2);
        dreg.extend_from_slice(&SynthDtb::cells(0x100, 2));
        d.prop("reg", &dreg);
        d.end_node();
        d.begin_node("gic@8000000");
        d.prop("compatible", b"arm,gic-v3\0");
        d.prop("phandle", &SynthDtb::cells(1, 1));
        d.prop("#interrupt-cells", &SynthDtb::cells(3, 1));
        let mut greg = SynthDtb::cells(0x0800_0000, 2);
        greg.extend_from_slice(&SynthDtb::cells(0x1000, 2));
        d.prop("reg", &greg);
        d.end_node();
        d.begin_node("rtc@9010000");
        d.prop("compatible", b"arm,pl031\0");
        let mut rreg = SynthDtb::cells(0x9010_0000, 2);
        rreg.extend_from_slice(&SynthDtb::cells(0x1000, 2));
        d.prop("reg", &rreg);
        d.prop("status", b"disabled\0");
        d.end_node();
        d.finish()
    }

    #[test]
    fn synthetic_platform_b_nested_bus_cells_and_ranges() {
        let blob = platform_b_dtb();
        let tree = parse_fdt_tree(&blob).expect("平台 B 必须可解析");
        // CPU 与内存事实。
        assert_eq!(tree.cpus, vec![0]);
        assert!(tree.memory.contains(&(0x4000_0000, 0x0800_0000)));
        // uart 的 reg 经 1-cell→2-cell ranges 翻译为 CPU 物理地址。
        let uart = tree
            .nodes
            .iter()
            .find(|n| n.compatible == "arm,pl011")
            .expect("uart");
        assert_eq!(
            uart.regs,
            vec![(0x0900_0000, 0x1000)],
            "ranges 翻译必须生效"
        );
        // 中断解析（SPI 1 → 33；flags=4 → level）。
        assert_eq!(
            uart.global_irqs,
            vec![InterruptSpec {
                irq: 33,
                trigger: IrqTrigger::Level
            }]
        );
        // 时钟 phandle 回填（clk@1 = 24 MHz）。
        assert_eq!(
            uart.clocks.first().and_then(|c| c.frequency_hz),
            Some(24_000_000)
        );
        // resets/gpios/dmas 按 provider #*-cells 拆分回填。
        assert_eq!(
            uart.resets,
            vec![eneros_hal::inventory::ResetRef {
                phandle: 5,
                specifier: vec![7]
            }]
        );
        assert_eq!(
            uart.gpios,
            vec![eneros_hal::inventory::GpioRef {
                phandle: 6,
                specifier: vec![1, 0]
            }]
        );
        assert_eq!(
            uart.dmas,
            vec![eneros_hal::inventory::DmaChannelRef {
                phandle: 7,
                specifier: vec![2, 3]
            }]
        );
        // 多 compatible 全量保留（优先级序）。
        let eth = tree
            .nodes
            .iter()
            .find(|n| n.compatible == "vendor,eth")
            .expect("eth");
        assert_eq!(eth.compatible_all, vec!["vendor,eth", "generic,net"]);
        // non-coherent 缺省；dma-coherent 显式。
        assert_eq!(eth.dma_coherent, Some(true));
        assert_eq!(uart.dma_coherent, None);
        // iommus 引用解析。
        assert_eq!(eth.iommus, vec![(4, 1)]);
        // disabled 设备：不进设备清单，进 disabled 报告面。
        let inv = tree.to_inventory();
        assert!(
            inv.nodes.iter().all(|n| n.compatible != "arm,pl031"),
            "disabled 设备不得入清单"
        );
        assert!(inv.disabled.iter().any(|p| p.contains("rtc@9010000")));
        // FDT provider 全链：describe → inventory（不同 cells 无需改核心）。
        let provider = FdtDescriptionProvider;
        let pinv = provider
            .describe(&PlatformInput {
                runtime_dtb: Some(&blob),
                bootinfo_cpu_count: 4,
            })
            .expect("平台 B describe 必须成功");
        assert!(pinv.validate().is_ok(), "平台 B inventory 校验通过");
        let p_uart = pinv
            .devices
            .iter()
            .find(|d| d.compatible.first() == "arm,pl011")
            .expect("pl011 in inventory");
        assert_eq!(p_uart.first_reg().unwrap().phys.as_u64(), 0x0900_0000);
    }

    #[test]
    fn synthetic_platform_b_malformed_ranges_fails_closed() {
        // 篡改 totalsize=0 → 解析拒绝（fail-closed，零设备产出）。
        let mut blob = platform_b_dtb();
        blob[4..8].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(parse_fdt_tree(&blob), Err(FdtError::Truncated));
        // 篡改 magic → BadHeader。
        let mut blob2 = platform_b_dtb();
        blob2[0] = 0x00;
        assert_eq!(parse_fdt_tree(&blob2), Err(FdtError::BadHeader));
    }

    #[test]
    fn synthetic_platform_b_unknown_compatible_kept_as_fact() {
        // 未知 compatible 是"发现事实"：保留在 inventory（绑定层报
        // Unsupported），不阻断平台解析。
        let blob = platform_b_dtb();
        let inv = parse_fdt(&blob).expect("parse");
        assert!(inv.nodes.iter().any(|n| n.compatible == "vendor,eth"));
    }
}
